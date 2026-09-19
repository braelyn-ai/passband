//! Durable reconnect work. Only tenant-sealed ciphertext is queued; OAuth codes,
//! PKCE verifiers and plaintext credentials never enter this table.
//!
//! THE SHAPE. A consent lands a row ([`enqueue`]) and answers the browser at
//! once with a cookie that names the row. A worker pass CLAIMS due rows with a
//! lease in one statement, drives the warden call holding no database
//! connection at all, and comes back to write the outcome. The status page
//! reads the row and nothing else.
//!
//! WHY NO CONNECTION IS HELD ACROSS THE CALL. The pool is four connections and
//! a rollout can take ten minutes. Four tenants reconnecting in the same
//! window - the documented trigger is a Testing-mode consent screen expiring
//! every refresh token on the same day - would otherwise pin every connection
//! "idle in transaction" and hang every other route on this service,
//! including the status page those same users are refreshing.
//!
//! WHY A LEASE RATHER THAN A LOCK. A transaction advisory lock is released
//! when the connection goes, which is the point of it, but it can only be held
//! by a connection, which is the thing this module refuses to hold. The lease
//! expires on its own a minute after the longest call the warden client will
//! wait for, so a process that dies mid-call leaves a row the next pass
//! repeats. Its unchanged deployment hash makes that repeat idempotent on the
//! cluster: same ciphertext, same pod template, no second rollout.
//!
//! RETRIES END. An outcome the warden did not settle (a timeout, a 5xx, a
//! dropped connection) is retried with a backoff, and after [`MAX_ATTEMPTS`]
//! the job gives up and says so. A job that retried forever would hold a
//! worker slot for every other tenant and keep a sealed credential at rest
//! indefinitely; the give-up NULLs the ciphertext like every other terminal
//! state does.
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio::task::JoinHandle;

use crate::{
    ControlState, config::RECONNECT_TIMEOUT, cookie, pages, sessions, warden::WardenError,
};

/// Rollouts one replica drives at once. A cap on cluster churn, not on
/// database connections: a job holds none while it waits on the warden.
pub const MAX_CONCURRENT: usize = 4;

/// Attempts a job gets before it gives up. With [`backoff_secs`] between them
/// that is about an hour and a half of waiting on top of the calls themselves,
/// which outlasts any node restart worth waiting out and is well inside the
/// day the browser's view of the job lives.
pub const MAX_ATTEMPTS: i32 = 8;

/// Longest wait between two attempts.
const BACKOFF_CAP_SECS: i32 = 30 * 60;

/// How long a claim lasts: the longest the warden client will wait, plus a
/// minute for the result write. A worker that is still running at the end of
/// its lease has hit the client timeout already and is about to write.
const LEASE_SECS: i32 = RECONNECT_TIMEOUT.as_secs() as i32 + 60;

/// A base64url token of 32 bytes, which is what [`crate::handlers`] mints.
const TOKEN_LEN: usize = 43;

/// What a row says about its job. Written and read by this module only, so
/// the Postgres `CHECK` and these four spellings are the same list; a fifth
/// state is a change here, not a string somewhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    /// Not yet settled: never tried, in flight, or waiting to be retried.
    Pending,
    /// The warden confirmed the rollout.
    Complete,
    /// The warden refused before installing anything: no such tenant, a
    /// custody or status disagreement, or a request this client should never
    /// have sent. Nothing changed and a retry would be refused the same way.
    Refused,
    /// This service stopped trying: [`MAX_ATTEMPTS`] unsettled outcomes, or a
    /// warden that refused our bearer, which no retry will fix.
    GaveUp,
}

impl JobStatus {
    fn as_db(self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Complete => "complete",
            JobStatus::Refused => "refused",
            JobStatus::GaveUp => "gave_up",
        }
    }

    fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => JobStatus::Pending,
            "complete" => JobStatus::Complete,
            "refused" => JobStatus::Refused,
            "gave_up" => JobStatus::GaveUp,
            _ => return None,
        })
    }
}

/// What the status page shows. A superset of [`JobStatus`]: two of these are
/// the same pending row seen at different ages, and two are not about the row
/// at all. [`pages::reconnect_result`] matches it exhaustively, so a state
/// added here is a page the compiler asks for rather than a fallthrough that
/// refreshes forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconnectView {
    /// Pending, untried or on its first attempt.
    Pending,
    /// Pending after at least one unsettled attempt.
    Retrying,
    /// The row could not be read. Not an outcome of the job.
    Checking,
    Complete,
    Refused,
    GaveUp,
    /// No cookie, a cookie naming no row, or a view past its day.
    Expired,
}

impl ReconnectView {
    /// Whether the page should reload itself: true exactly for the states the
    /// worker can still move.
    pub fn refreshes(self) -> bool {
        matches!(
            self,
            ReconnectView::Pending | ReconnectView::Retrying | ReconnectView::Checking
        )
    }
}

/// Register a browser's access to the tenant's pending operation. Concurrent
/// consent flows join that operation instead of racing credential replacements.
///
/// THE NEWEST CONSENT WINS. A second consent while a job is pending carries a
/// fresh refresh token, and it may be the only live one: somebody impatient
/// with a stuck job revokes Passband in their Google account (which kills
/// every earlier token) and re-consents. Keeping the first ciphertext would
/// install a dead credential and report success. So the row takes the new
/// sealed blob and starts its attempt count over; a worker mid-flight on the
/// old blob finds its row changed underneath it and leaves it pending for the
/// new one (see [`run_job`]).
pub async fn enqueue(
    state: &ControlState,
    token: &str,
    label: &str,
    email: &str,
    ciphertext: &str,
) -> anyhow::Result<()> {
    let id = sessions::fingerprint(token);
    let mut client = state.store().client().await?;
    let tx = client.transaction().await?;
    let job: String = tx
        .query_one(
            "INSERT INTO reconnect_jobs (id, label, account_email, ciphertext) VALUES ($1,$2,$3,$4)
             ON CONFLICT (label) WHERE status = 'pending' DO UPDATE
                 SET ciphertext = EXCLUDED.ciphertext,
                     account_email = EXCLUDED.account_email,
                     attempts = 0,
                     retry_at = now()
             RETURNING id",
            &[&id, &label, &email, &ciphertext],
        )
        .await?
        .get(0);
    tx.execute(
        "INSERT INTO reconnect_views (token_hash, job_id) VALUES ($1,$2)",
        &[&id, &job],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The answer to a consent that was queued: over to the status page, with the
/// cookie that lets this browser read it.
pub fn started(token: &str, secure: bool) -> Response {
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/reconnect/status".to_string()),
            (
                header::SET_COOKIE,
                cookie::set_reconnect_cookie(token, secure),
            ),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
        ],
    )
        .into_response()
}

fn browser_token(headers: &HeaderMap) -> Option<&str> {
    let value = cookie::reconnect_from_header(headers.get(header::COOKIE)?.to_str().ok()?)?;
    (value.len() == TOKEN_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then_some(value)
}

pub async fn status(State(state): State<ControlState>, headers: HeaderMap) -> Response {
    let Some(token) = browser_token(&headers) else {
        return pages::reconnect_result(ReconnectView::Expired);
    };
    let row = async {
        let client = state.store().client().await?;
        client
            .query_opt(
                "SELECT j.status, j.attempts FROM reconnect_views v
                 JOIN reconnect_jobs j ON j.id = v.job_id
                 WHERE v.token_hash = $1 AND v.expires_at > now()",
                &[&sessions::fingerprint(token)],
            )
            .await
            .map_err(anyhow::Error::from)
    }
    .await;
    let view = match row {
        Ok(Some(row)) => {
            let status: &str = row.get(0);
            let attempts: i32 = row.get(1);
            match JobStatus::from_db(status) {
                Some(JobStatus::Pending) if attempts > 0 => ReconnectView::Retrying,
                Some(JobStatus::Pending) => ReconnectView::Pending,
                Some(JobStatus::Complete) => ReconnectView::Complete,
                Some(JobStatus::Refused) => ReconnectView::Refused,
                Some(JobStatus::GaveUp) => ReconnectView::GaveUp,
                None => {
                    // The CHECK constraint makes this a schema edit nobody
                    // told this module about. Say so, and keep the page
                    // moving rather than freezing it on a guess.
                    tracing::error!(
                        status,
                        "reconnect row carries a status this build does not know"
                    );
                    ReconnectView::Checking
                }
            }
        }
        Ok(None) => ReconnectView::Expired,
        // An unavailable database is not an outcome of the operation.
        Err(e) => {
            tracing::error!(error = %e, "reading reconnect progress failed");
            ReconnectView::Checking
        }
    };
    pages::reconnect_result(view)
}

/// One row, claimed. Everything [`run_job`] needs, so it opens no connection
/// until it has something to write.
struct ClaimedJob {
    id: String,
    label: String,
    email: String,
    ciphertext: Option<String>,
    /// Finished attempts BEFORE this one.
    attempts: i32,
}

/// One worker pass: claim what is due, up to the free slots, and start a job
/// per row. Called on a ticker, at startup, and right after a consent is
/// queued.
///
/// The handles are returned rather than awaited. The ticker must not park on
/// the slowest rollout - that is how a fifth tenant waits ten minutes with
/// three slots idle - so it drops them, which detaches the jobs. A test that
/// needs the outcome awaits them.
pub async fn run_pending(state: &ControlState) -> Vec<JoinHandle<()>> {
    match run_pending_inner(state).await {
        Ok(handles) => handles,
        Err(e) => {
            tracing::error!(error = %e, "reconnect worker pass failed");
            Vec::new()
        }
    }
}

async fn run_pending_inner(state: &ControlState) -> anyhow::Result<Vec<JoinHandle<()>>> {
    // Slots FIRST, then exactly that many rows. A row claimed with no slot to
    // run it would sit leased and idle for the length of the lease, which is
    // worse than leaving it for the next pass.
    let mut permits = Vec::with_capacity(MAX_CONCURRENT);
    while permits.len() < MAX_CONCURRENT {
        match state.reconnect_slot() {
            Some(permit) => permits.push(permit),
            None => break,
        }
    }
    if permits.is_empty() {
        return Ok(Vec::new());
    }
    let client = state.store().client().await?;
    // The claim IS the selection. `SKIP LOCKED` keeps two replicas' passes off
    // the same rows, and the lease keeps the next pass off them once this one
    // has committed and let go of the row lock.
    let rows = client
        .query(
            "UPDATE reconnect_jobs
             SET lease_until = clock_timestamp() + $2::int4 * interval '1 second'
             WHERE id IN (
                 SELECT id FROM reconnect_jobs
                 WHERE status = 'pending' AND retry_at <= now()
                   AND (lease_until IS NULL OR lease_until < now())
                 ORDER BY created_at
                 LIMIT $1
                 FOR UPDATE SKIP LOCKED)
             RETURNING id, label, account_email, ciphertext, attempts",
            &[&(permits.len() as i64), &LEASE_SECS],
        )
        .await?;
    drop(client);
    // Slots the claim did not fill go back on the drop of the zipped tail.
    let handles = rows
        .into_iter()
        .zip(permits)
        .map(|(row, permit)| {
            let job = ClaimedJob {
                id: row.get(0),
                label: row.get(1),
                email: row.get(2),
                ciphertext: row.get(3),
                attempts: row.get(4),
            };
            let state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(e) = run_job(&state, job).await {
                    tracing::error!(error = %e, "reconnect worker could not save its result");
                }
            })
        })
        .collect();
    Ok(handles)
}

/// Seconds to wait before attempt `attempt + 1`, after `attempt` has failed:
/// a minute, doubling, capped at half an hour.
fn backoff_secs(attempt: i32) -> i32 {
    let doublings = attempt.clamp(1, 16) - 1;
    (60i32 << doublings.min(10)).min(BACKOFF_CAP_SECS)
}

async fn run_job(state: &ControlState, job: ClaimedJob) -> anyhow::Result<()> {
    let attempt = job.attempts + 1;
    let Some(ciphertext) = job.ciphertext.as_deref() else {
        // A pending row with nothing to install is a bug in whatever wrote it.
        tracing::error!(label = %job.label, "reconnect row is pending with no ciphertext");
        return save(state, &job, "", JobStatus::GaveUp, 0).await;
    };
    let result = state
        .warden()
        .reconnect_credentials(&job.label, &job.email, ciphertext)
        .await;
    let (status, retry_in) = match result {
        Ok(()) => (JobStatus::Complete, 0),
        // Refused before any credential was installed: nothing changed and the
        // same request would be refused the same way.
        Err(
            WardenError::LabelTaken
            | WardenError::NotFound
            | WardenError::LabelRefused
            | WardenError::NotCiphertext,
        ) => {
            tracing::error!(label = %job.label, "reconnect: the warden refused the credential");
            (JobStatus::Refused, 0)
        }
        // A deployment misconfiguration. Nothing will land until an operator
        // fixes it, and retrying would only say so every minute.
        Err(WardenError::Unauthorized) => {
            tracing::error!(label = %job.label, "reconnect: the warden refused our bearer");
            (JobStatus::GaveUp, 0)
        }
        // A timeout or 5xx cannot settle whether the credential landed. Retry
        // the same sealed credential without asking for Google consent again -
        // up to a point.
        Err(e) if attempt >= MAX_ATTEMPTS => {
            tracing::error!(label = %job.label, error = %e, attempt, "reconnect gave up");
            (JobStatus::GaveUp, 0)
        }
        Err(e) => {
            let retry_in = backoff_secs(attempt);
            tracing::warn!(label = %job.label, error = %e, attempt, retry_in, "reconnect will retry automatically");
            (JobStatus::Pending, retry_in)
        }
    };
    save(state, &job, ciphertext, status, retry_in).await
}

/// Write an attempt's outcome, and release the lease either way.
///
/// GUARDED ON THE CIPHERTEXT THIS ATTEMPT INSTALLED. If a newer consent
/// replaced the blob while this call was in flight ([`enqueue`]), this outcome
/// is about a credential the row no longer holds: the row stays pending, at
/// its reset attempt count, for the next pass to install the new one.
async fn save(
    state: &ControlState,
    job: &ClaimedJob,
    installed: &str,
    status: JobStatus,
    retry_in: i32,
) -> anyhow::Result<()> {
    let client = state.store().client().await?;
    let row = client
        .query_opt(
            "UPDATE reconnect_jobs SET
                 lease_until = NULL,
                 status = CASE WHEN ciphertext = $2 THEN $3::text ELSE status END,
                 attempts = CASE WHEN ciphertext = $2 THEN attempts + 1 ELSE attempts END,
                 retry_at = CASE WHEN ciphertext = $2
                     THEN clock_timestamp() + $4::int4 * interval '1 second' ELSE retry_at END,
                 ciphertext = CASE WHEN ciphertext = $2 AND $3 <> 'pending' THEN NULL ELSE ciphertext END
             WHERE id = $1 AND status = 'pending'
             RETURNING attempts",
            &[&job.id, &installed, &status.as_db(), &retry_in],
        )
        .await?;
    match row {
        Some(row) if row.get::<_, i32>(0) == job.attempts + 1 => {
            tracing::info!(label = %job.label, status = status.as_db(), "reconnect progress saved");
        }
        Some(_) => {
            tracing::info!(label = %job.label, "reconnect outcome superseded by a newer consent");
        }
        None => {
            tracing::warn!(label = %job.label, "reconnect row was gone before its outcome was saved");
        }
    }
    Ok(())
}

/// Drop views past their day and finished jobs nobody can still see. On the
/// session sweeper's cadence, not the worker's: this is housekeeping, and a
/// pass that runs every five seconds should do nothing when there is nothing
/// to do.
pub async fn sweep(state: &ControlState) {
    if let Err(e) = sweep_inner(state).await {
        tracing::error!(error = %e, "reconnect sweep failed");
    }
}

async fn sweep_inner(state: &ControlState) -> anyhow::Result<()> {
    let client = state.store().client().await?;
    client
        .execute("DELETE FROM reconnect_views WHERE expires_at < now()", &[])
        .await?;
    client
        .execute(
            "DELETE FROM reconnect_jobs WHERE status <> 'pending'
             AND created_at < now() - interval '1 day'
             AND NOT EXISTS (SELECT 1 FROM reconnect_views WHERE job_id = reconnect_jobs.id)",
            &[],
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_a_minute_and_caps_at_half_an_hour() {
        assert_eq!(backoff_secs(1), 60);
        assert_eq!(backoff_secs(2), 120);
        assert_eq!(backoff_secs(5), 960);
        assert_eq!(backoff_secs(6), BACKOFF_CAP_SECS);
        assert_eq!(backoff_secs(MAX_ATTEMPTS), BACKOFF_CAP_SECS);
        // Nonsense inputs stay finite.
        assert_eq!(backoff_secs(0), 60);
        assert_eq!(backoff_secs(i32::MAX), BACKOFF_CAP_SECS);
    }

    #[test]
    fn every_status_round_trips_through_its_column_spelling() {
        for status in [
            JobStatus::Pending,
            JobStatus::Complete,
            JobStatus::Refused,
            JobStatus::GaveUp,
        ] {
            assert_eq!(JobStatus::from_db(status.as_db()), Some(status));
        }
        assert_eq!(JobStatus::from_db("complet"), None);
    }

    #[test]
    fn only_the_states_the_worker_can_move_refresh() {
        assert!(ReconnectView::Pending.refreshes());
        assert!(ReconnectView::Retrying.refreshes());
        assert!(ReconnectView::Checking.refreshes());
        for terminal in [
            ReconnectView::Complete,
            ReconnectView::Refused,
            ReconnectView::GaveUp,
            ReconnectView::Expired,
        ] {
            assert!(!terminal.refreshes(), "{terminal:?}");
        }
    }

    #[test]
    fn the_lease_outlasts_the_client_timeout() {
        assert!(u64::try_from(LEASE_SECS).unwrap() > RECONNECT_TIMEOUT.as_secs());
    }
}
