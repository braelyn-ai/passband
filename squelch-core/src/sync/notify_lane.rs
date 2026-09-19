//! Independent notification inference with bounded concurrency and model budgets.
//! Auth qualifies at any score. Other mail uses the notification threshold.
//! Failures remain unavailable for the full agent to rescue; there is no seed fallback.
//! The store supplies immutable arrival eligibility and arbitrates duplicate delivery.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use super::{BudgetGate, BudgetLedger, CapKind, NOTIFY_FAST_BUDGET_KEY, NOTIFY_USAGE_CATEGORY};
use crate::config::{NotifyConfig, ResolvedLlm, Stage2Provider};
use crate::metrics::{NotifyDecision, NotifyLane as LaneLabel, SyncMetrics};
use crate::store::{NewNotifyDecision, Store, TriagedMessage};
use crate::triage::events::{self, EventContext, Refusal};
use crate::triage::llm::{self, LlmOutcome};
use crate::triage::notify_llm::{self, NotifyInput};

use crate::types::{AccountId, SenderRule, Sensitivity, Tier};

const DISABLE_AFTER_CONFIG_FAILURE: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub struct Candidate {
    message_id: i64,
    thread_id: String,
    sender: String,
    subject: String,
    body: String,
    is_known_contact: bool,
    sender_preferences: Option<String>,
}

impl Candidate {
    /// Build the fast lane's bounded input from a durable job's account-scoped context.
    pub fn from_agent_context(
        context: &crate::store::agent_triage::AgentContext,
        eligible_at: Option<DateTime<Utc>>,
        cfg: &NotifyConfig,
    ) -> Option<Self> {
        eligible_at?;
        let message = &context.message;
        if message.is_sent || message.is_spam {
            return None;
        }
        Some(Self {
            message_id: message.id,
            thread_id: message.thread_id.clone(),
            sender: message.from_addr.clone(),
            subject: message.subject.clone(),
            body: crate::text::truncate_chars(&message.body, cfg.max_body_chars.saturating_add(1)),
            is_known_contact: context.sender_is_contact,
            sender_preferences: if context.matched_rules.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&context.matched_rules).expect("rules are JSON"))
            },
        })
    }
}

pub fn candidate_from_context(
    context: &crate::store::agent_triage::AgentContext,
    cfg: &NotifyConfig,
) -> Option<Candidate> {
    let eligible_at = context
        .message
        .notify_eligible_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));
    Candidate::from_agent_context(context, eligible_at, cfg)
}

pub fn candidate(
    triaged: &TriagedMessage,
    message_id: i64,
    rules: &[SenderRule],
    cfg: &NotifyConfig,
    known_contact: impl FnOnce(&str) -> bool,
) -> Option<Candidate> {
    if triaged.message.is_sent || triaged.message.is_spam {
        return None;
    }
    triaged.notify_eligible_at?;
    let sender_preferences =
        crate::triage::rules::match_sender_rule(&triaged.message.from_addr, rules).map(|r| {
            format!(
                "Disposition: {:?}. User preference: {}",
                r.disposition, r.want_text
            )
        });
    Some(Candidate {
        message_id,
        thread_id: triaged.message.thread_id.clone(),
        sender: triaged.message.from_addr.clone(),
        subject: triaged.message.subject.clone(),
        body: crate::text::truncate_chars(
            &triaged.message.body,
            cfg.max_body_chars.saturating_add(1),
        ),
        is_known_contact: known_contact(&triaged.message.from_addr),
        sender_preferences,
    })
}

struct Verdict<'a> {
    importance: u8,
    is_auth: bool,
    one_line: &'a str,
    model_used: &'a str,
}

pub struct NotifyLane<S: Store> {
    store: Arc<S>,
    http: reqwest::Client,
    cfg: NotifyConfig,
    llm: Option<ResolvedLlm>,
    metrics: Arc<SyncMetrics>,
    account_id: AccountId,
    permits: tokio::sync::Semaphore,
    disabled_until: std::sync::Mutex<Option<Instant>>,
    in_flight: std::sync::Mutex<std::collections::HashSet<i64>>,
    warn_days: Arc<std::sync::Mutex<super::WarnDays>>,
}

impl<S: Store + 'static> NotifyLane<S> {
    #[allow(clippy::too_many_arguments)] // one struct's fields, spelled out once
    pub(super) fn new(
        store: Arc<S>,
        http: reqwest::Client,
        mut cfg: NotifyConfig,
        _known_contact_floor: u8,
        llm: Option<ResolvedLlm>,
        metrics: Arc<SyncMetrics>,
        account_id: AccountId,
        warn_days: Arc<std::sync::Mutex<super::WarnDays>>,
    ) -> Self {
        if matches!(&llm, Some(l) if l.provider == Stage2Provider::Anthropic
                        && llm::is_gateway_url(&l.url))
            && let Some(q) = llm::qualify_gateway_model(&cfg.model)
        {
            cfg.model = q;
        }
        let permits = tokio::sync::Semaphore::new(cfg.fast_concurrency.max(1));
        Self {
            store,
            http,
            cfg,
            llm,
            metrics,
            account_id,
            permits,
            disabled_until: std::sync::Mutex::new(None),
            in_flight: std::sync::Mutex::new(std::collections::HashSet::new()),
            warn_days,
        }
    }

    pub async fn run(self: Arc<Self>, c: Candidate) -> crate::Result<()> {
        let message_id = c.message_id;

        let Some(_claim) = InFlight::claim(&self.in_flight, message_id) else {
            return Ok(());
        };

        if self
            .store
            .notify_decision_exists(self.account_id, message_id, LaneLabel::Fast)?
        {
            return Ok(());
        }

        let eligible_at = self.store.notify_eligible_at(self.account_id, message_id)?;
        let Some(eligible_at) = eligible_at else {
            return Ok(());
        };

        self.clone().run_model(c, eligible_at).await
    }

    /// A completed triage decision can rescue a declined or unavailable fast
    /// assessment. Both paths share qualification and the store's arrival dedup.
    pub fn request_assessed(
        &self,
        context: &crate::store::agent_triage::AgentContext,
        is_auth: bool,
        advice: &crate::triage::decision::NotificationAdvice,
        model: &str,
    ) -> crate::Result<()> {
        if context.message.is_sent || context.message.is_spam {
            return Ok(());
        }
        let Some(eligible_at) = self
            .store
            .notify_eligible_at(self.account_id, context.message.id)?
        else {
            return Ok(());
        };
        let row = ModelRow {
            message_id: context.message.id,
            thread_id: context.message.thread_id.clone(),
            sender: context.message.from_addr.clone(),
            eligible_at,
        };
        let one_line = crate::text::truncate_chars(&advice.body, 160);
        self.emit(
            &row,
            Verdict {
                importance: advice.importance,
                is_auth,
                one_line: &one_line,
                model_used: model,
            },
            Utc::now(),
            LaneLabel::Deliberate,
        )
    }

    async fn run_model(
        self: Arc<Self>,
        c: Candidate,
        eligible_at: DateTime<Utc>,
    ) -> crate::Result<()> {
        let Candidate {
            message_id,
            thread_id,
            sender,
            subject,
            body,
            is_known_contact,
            sender_preferences,
        } = c;

        let m = ModelRow {
            message_id,
            thread_id,
            sender,
            eligible_at,
        };

        let Some(llm) = self.llm.clone() else {
            self.record(
                m.message_id,
                NotifyDecision::Unavailable,
                None,
                None,
                m.eligible_at,
                Utc::now(),
            )?;
            return Ok(());
        };

        if !self.cfg.fast_enabled || self.is_disabled() {
            self.record(
                m.message_id,
                NotifyDecision::Unavailable,
                None,
                None,
                m.eligible_at,
                Utc::now(),
            )?;
            return Ok(());
        }

        let day = Utc::now().format("%Y-%m-%d").to_string();
        match self.budget().gate(
            NOTIFY_FAST_BUDGET_KEY,
            &day,
            self.cfg.daily_cap,
            CapKind::NotifyFast,
            "notify fast lane",
            "remaining messages",
        ) {
            BudgetGate::Proceed => {}
            BudgetGate::SkipRow => {
                return Err(crate::error::CoreError::InvalidInput(
                    "notification_budget_store_unavailable".into(),
                ));
            }
            BudgetGate::Exhausted => {
                self.record(
                    m.message_id,
                    NotifyDecision::Unavailable,
                    None,
                    None,
                    m.eligible_at,
                    Utc::now(),
                )?;
                return Ok(());
            }
        }

        let Ok(_permit) = self.permits.acquire().await else {
            self.record(
                m.message_id,
                NotifyDecision::Unavailable,
                None,
                None,
                m.eligible_at,
                Utc::now(),
            )?;
            return Ok(());
        };

        let input = NotifyInput {
            from_addr: &m.sender,
            subject: &subject,
            body: &body,
            is_known_contact,
            sender_preferences: sender_preferences.as_deref(),
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(self.cfg.fast_timeout_secs),
            notify_llm::classify_at(
                &self.http,
                &llm.url,
                &llm.api_key,
                &self.cfg,
                llm.provider,
                &input,
            ),
        )
        .await;

        if let Ok(Ok(response)) = &outcome
            && let Some(u) = response.usage()
            && let Err(e) = self.store.extract_bump_usage(
                self.account_id,
                &day,
                NOTIFY_USAGE_CATEGORY,
                u.into(),
            )
        {
            eprintln!("squelch: notify usage ledger write failed ({e})");
        }
        let now = Utc::now();
        match outcome {
            Ok(Ok(LlmOutcome::Ok(out, _))) => {
                let importance = out.notify_importance as u8;
                let one_line = crate::text::truncate_chars(&out.one_line, 160);
                self.store.record_notification_assessment(
                    self.account_id,
                    m.message_id,
                    LaneLabel::Fast,
                    &crate::store::NotificationAssessment {
                        is_auth: out.is_auth,
                        importance,
                        one_line: one_line.clone(),
                        reason: out.reason.clone(),
                        model: self.cfg.model.clone(),
                        prompt_version: notify_llm::PROMPT_VERSION.into(),
                        assessed_at: now,
                    },
                )?;
                self.emit(
                    &m,
                    Verdict {
                        importance,
                        is_auth: out.is_auth,
                        one_line: &one_line,
                        model_used: &self.cfg.model,
                    },
                    now,
                    LaneLabel::Fast,
                )?;
            }
            Ok(Ok(LlmOutcome::Failed(kind, _))) if llm::is_config_failure(&kind) => {
                self.disable_for(DISABLE_AFTER_CONFIG_FAILURE);
                self.budget()
                    .refund(NOTIFY_FAST_BUDGET_KEY, &day, "notify fast lane");
                self.metrics.record_llm_config_failure();
                if self.budget().warn_once(CapKind::NotifyFastConfig, &day) {
                    eprintln!(
                        "squelch: notify fast lane config-level failure ({kind}); pausing the \
                         lane for 10 minutes (the triage passes still notify)"
                    );
                }
                self.record(
                    m.message_id,
                    NotifyDecision::Unavailable,
                    None,
                    None,
                    m.eligible_at,
                    now,
                )?;
            }
            Ok(Ok(LlmOutcome::Refused(_)))
            | Ok(Ok(LlmOutcome::Failed(_, _)))
            | Ok(Err(_))
            | Err(_) => {
                self.record(
                    m.message_id,
                    NotifyDecision::Unavailable,
                    None,
                    None,
                    m.eligible_at,
                    now,
                )?;
            }
        }
        Ok(())
    }

    fn emit(
        &self,
        m: &ModelRow,
        v: Verdict<'_>,
        now: DateTime<Utc>,
        lane: LaneLabel,
    ) -> crate::Result<()> {
        let ctx = EventContext {
            account_id: self.account_id,
            message_id: m.message_id,
            thread_id: &m.thread_id,
            sender: &m.sender,
            one_line: v.one_line,
            notify_eligible_at: Some(m.eligible_at),
            sensitivity: Sensitivity::Normal,
            is_sent: false,
            is_spam: false,
            rule: None,
            tier: Tier::Signal,
            importance: v.importance,
            deadline: None,
        };
        match events::notification_event(&ctx, v.is_auth, &self.cfg, now) {
            Ok(ev) => match self.store.append_event(&ev) {
                Ok(Some(_)) => self.record_for_lane(
                    lane,
                    m.message_id,
                    NotifyDecision::Sent,
                    Some(v.importance),
                    Some(v.model_used),
                    m.eligible_at,
                    now,
                ),
                Ok(None) => self.record_for_lane(
                    lane,
                    m.message_id,
                    if self
                        .store
                        .message_has_event(self.account_id, m.message_id)?
                    {
                        NotifyDecision::WouldSend
                    } else {
                        NotifyDecision::Suppressed
                    },
                    Some(v.importance),
                    Some(v.model_used),
                    m.eligible_at,
                    now,
                ),
                Err(error) => Err(error),
            },
            Err(Refusal::NotWorthy) => self.record_for_lane(
                lane,
                m.message_id,
                NotifyDecision::DeclinedByModel,
                Some(v.importance),
                Some(v.model_used),
                m.eligible_at,
                now,
            ),
            Err(Refusal::Suppressed) => self.record_for_lane(
                lane,
                m.message_id,
                NotifyDecision::Suppressed,
                Some(v.importance),
                Some(v.model_used),
                m.eligible_at,
                now,
            ),
            Err(Refusal::Expired) => {
                let decision = if self
                    .store
                    .message_has_event(self.account_id, m.message_id)?
                {
                    NotifyDecision::WouldSend
                } else {
                    NotifyDecision::Expired
                };
                self.record_for_lane(
                    lane,
                    m.message_id,
                    decision,
                    Some(v.importance),
                    Some(v.model_used),
                    m.eligible_at,
                    now,
                )
            }
        }
    }

    fn record(
        &self,
        message_id: i64,
        decision: NotifyDecision,
        notify_importance: Option<u8>,
        model_used: Option<&str>,
        eligible_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> crate::Result<()> {
        self.record_for_lane(
            LaneLabel::Fast,
            message_id,
            decision,
            notify_importance,
            model_used,
            eligible_at,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_for_lane(
        &self,
        lane: LaneLabel,
        message_id: i64,
        decision: NotifyDecision,
        notify_importance: Option<u8>,
        model_used: Option<&str>,
        eligible_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> crate::Result<()> {
        let latency = (now - eligible_at)
            .num_milliseconds()
            .clamp(0, i64::from(u32::MAX)) as u32;
        let row = NewNotifyDecision {
            account_id: self.account_id,
            message_id,
            lane,
            decision,
            notify_importance,
            model_used: model_used.map(str::to_string),
            latency_ms: Some(latency),
        };
        if self.store.record_notify_decision(&row)? {
            self.metrics.record_notify(lane, decision);
            if decision == NotifyDecision::Sent && lane == LaneLabel::Fast {
                self.metrics.observe_notify_fast(latency as f64 / 1000.0);
            }
        }
        Ok(())
    }

    fn is_disabled(&self) -> bool {
        let Ok(until) = self.disabled_until.lock() else {
            return false;
        };
        until.is_some_and(|t| Instant::now() < t)
    }

    fn disable_for(&self, d: Duration) {
        if let Ok(mut until) = self.disabled_until.lock() {
            *until = Some(Instant::now() + d);
        }
    }

    fn budget(&self) -> BudgetLedger<'_, S> {
        BudgetLedger {
            store: &*self.store,
            account_id: self.account_id,
            warn_days: &self.warn_days,
        }
    }
}

struct InFlight<'a> {
    set: &'a std::sync::Mutex<std::collections::HashSet<i64>>,
    message_id: i64,
}

impl<'a> InFlight<'a> {
    fn claim(
        set: &'a std::sync::Mutex<std::collections::HashSet<i64>>,
        message_id: i64,
    ) -> Option<Self> {
        let mut set_guard = set.lock().unwrap_or_else(|p| p.into_inner());
        if !set_guard.insert(message_id) {
            return None;
        }
        drop(set_guard);
        Some(Self { set, message_id })
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut set = self.set.lock().unwrap_or_else(|p| p.into_inner());
        set.remove(&self.message_id);
    }
}

struct ModelRow {
    message_id: i64,
    thread_id: String,
    sender: String,
    eligible_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Stage1Config, Stage2Provider};
    use crate::store::SqliteStore;
    use crate::sync::ingest::{RawFetched, ingest_with_rules};
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ---- a loopback model mock that RECORDS its requests --------------------
    //
    // Recording is not decoration here. "One attempt, no retry" and "no request
    // at all while the lane is parked" are properties about the NUMBER of
    // requests, and a mock that accepts exactly one connection cannot tell
    // "asked once" from "asked three times and the mock hung up". And the model
    // id the lane actually PUT ON THE WIRE is a property of the request body:
    // asserting it only on the ledger row is how a lane that 400s on every
    // gateway deployment passes its own suite.

    /// Read one whole HTTP request: headers, then exactly `content-length`
    /// bytes. A single `read` would truncate a 6 KB prompt at the first segment
    /// boundary and every body assertion would pass or fail by luck. Same shape
    /// as `notify_llm`'s, and here for the same reason.
    async fn read_request(sock: &mut tokio::net::TcpStream) -> String {
        let mut buf: Vec<u8> = Vec::with_capacity(16384);
        let mut chunk = [0u8; 4096];
        loop {
            let n = match sock.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).to_string();
            let Some(head_end) = text.find("\r\n\r\n") else {
                continue;
            };
            let want: usize = text[..head_end]
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse().ok())
                })
                .unwrap_or(0);
            if buf.len() >= head_end + 4 + want {
                break;
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    /// What a mock does after it has RECORDED a request, which is the axis the
    /// race tests need: a request that has landed but not yet been answered is
    /// exactly the window a mid-call seal, or a second overlapping `run`, lives
    /// in.
    #[derive(Clone)]
    enum Hold {
        /// Answer at once.
        None,
        /// Accept and answer nothing, ever: the timeout's test.
        Forever,
        /// Answer once the watch flips true, so a test can act while the call is
        /// demonstrably in flight rather than after a sleep it hopes is long
        /// enough.
        Until(tokio::sync::watch::Receiver<bool>),
    }

    /// A mock that answers every request with `(status, body)` and records each
    /// one it saw, whole. `hang` makes it accept and never answer, which is how
    /// the timeout is tested without a sleep standing in for an assertion.
    async fn mock(
        status: u16,
        body: impl Into<String>,
        hang: bool,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        mock_held(status, body, if hang { Hold::Forever } else { Hold::None }).await
    }

    /// Bounded polling with a deadline. NOT a sleep standing in for an
    /// assertion: the condition is the assertion and the deadline only bounds
    /// how long a broken build takes to say so.
    async fn until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("{what}");
    }

    async fn mock_held(
        status: u16,
        body: impl Into<String>,
        hold: Hold,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let body: Arc<str> = Arc::from(body.into());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let sink = sink.clone();
                let body = body.clone();
                let hold = hold.clone();
                tokio::spawn(async move {
                    let req = read_request(&mut sock).await;
                    sink.lock().unwrap().push(req);
                    match hold {
                        Hold::None => {}
                        // Hold the socket open, answering nothing, until the
                        // caller's timeout fires and drops it.
                        Hold::Forever => std::future::pending::<()>().await,
                        Hold::Until(mut rx) => {
                            while !*rx.borrow_and_update() {
                                if rx.changed().await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    /// An Anthropic-shaped verdict at `importance`, with a one_line the tests
    /// assert on so an event carrying the SEED's line instead would fail.
    ///
    /// BUILT WITH `serde_json`, not a raw string literal: the payload is JSON
    /// nested inside a JSON string, and a hand-escaped version of that is how a
    /// test ends up asserting on a parse failure it mistook for a verdict.
    fn verdict(importance: i64) -> String {
        let verdict = serde_json::json!({
            "notify_importance": importance,
            "is_auth": false,
            "reason": "Notification assessment",
            "one_line": "The model wrote this line",
        })
        .to_string();
        serde_json::json!({
            "content": [{"type": "text", "text": verdict}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 900, "output_tokens": 20},
        })
        .to_string()
    }

    fn cfg() -> NotifyConfig {
        NotifyConfig {
            // One second, so the timeout test finishes in a second rather than
            // eight, and is still an order of magnitude above loopback noise.
            fast_timeout_secs: 1,
            ..NotifyConfig::default()
        }
    }

    /// A lane over `store`, pointed at `url` (or with no model at all).
    fn lane(
        store: &Arc<SqliteStore>,
        acct: AccountId,
        url: Option<&str>,
        cfg: NotifyConfig,
    ) -> Arc<NotifyLane<SqliteStore>> {
        let llm = url.map(|u| ResolvedLlm {
            api_key: "sk-test".to_string(),
            provider: Stage2Provider::Anthropic,
            url: u.to_string(),
        });
        Arc::new(NotifyLane::new(
            store.clone(),
            reqwest::Client::new(),
            cfg,
            Config::default().stage1.known_contact_importance,
            llm,
            SyncMetrics::new(),
            acct,
            Arc::new(std::sync::Mutex::new(super::super::WarnDays::default())),
        ))
    }

    /// Ingest one RFC822 through the real pipeline and the real store, stamped
    /// eligible at `now` exactly as the incremental path stamps it. Returns the
    /// message id and the candidate the lane would have been spawned on.
    fn ingest(
        store: &Arc<SqliteStore>,
        acct: AccountId,
        msgid: &str,
        eml: &str,
        now: DateTime<Utc>,
        cfg: &NotifyConfig,
    ) -> (i64, Option<Candidate>) {
        let f = RawFetched {
            account_id: acct,
            gmail_msg_id: msgid.to_string(),
            gmail_thread_id: None,
            raw: eml.as_bytes().to_vec(),
            internal_date: Some(now),
            is_sent: false,
            is_spam: false,
            account_addr: "me@example.com".to_string(),
        };
        let rules = store.list_sender_rules(acct).unwrap();
        let mut triaged = ingest_with_rules(&f, &Stage1Config::default(), now, &rules, |addr| {
            store.is_known_contact(acct, addr).unwrap_or(false)
        });
        triaged.notify_eligible_at = super::super::notify_eligible_stamp(
            &triaged,
            super::super::IngestOrigin::Incremental,
            cfg,
            now,
        );
        let id = store.ingest_message(&triaged).unwrap();
        let c = candidate(&triaged, id, &rules, cfg, |addr| {
            store.is_known_contact(acct, addr).unwrap_or(false)
        });
        (id, c)
    }

    fn store() -> (Arc<SqliteStore>, AccountId) {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let acct = store.ensure_account("me@example.com").unwrap();
        (store, acct)
    }

    /// A plain personal note from a stranger: normal sensitivity, not spam, and
    /// nowhere near confident, so the SEED never decides it and only the model's
    /// score can.
    fn note_eml(at: DateTime<Utc>) -> String {
        format!(
            "From: Dana <dana@elsewhere.example>\r\n\
             To: me@example.com\r\n\
             Subject: quick question about thursday\r\n\
             Date: {}\r\n\
             \r\n\
             Are you free thursday afternoon? Let me know either way.\r\n",
            at.to_rfc2822()
        )
    }

    /// The one ledger row this lane wrote for `message_id`.
    fn ledger(
        store: &SqliteStore,
        acct: AccountId,
        message_id: i64,
    ) -> Option<crate::store::NotifyDecisionRow> {
        store
            .notify_decisions_since(acct, Utc::now() - chrono::Duration::hours(1), 100)
            .unwrap()
            .into_iter()
            .find(|r| r.message_id == message_id)
    }

    #[tokio::test]
    async fn a_low_score_is_declined_and_appends_nothing() {
        let (store, acct) = store();
        let now = Utc::now();
        let v = verdict(20);
        let (url, _) = mock(200, v, false).await;
        let (mid, c) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());

        lane(&store, acct, Some(&url), cfg())
            .run(c.unwrap())
            .await
            .unwrap();

        let row = ledger(&store, acct, mid).expect("a declined row is still a row");
        assert_eq!(row.decision, NotifyDecision::DeclinedByModel);
        assert_eq!(
            row.notify_importance,
            Some(20),
            "the score is stored even when it loses: it is the label the \
             threshold moves from"
        );
        assert!(store.events_after(acct, 0, 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_500_is_unavailable_after_exactly_one_request() {
        let (store, acct) = store();
        let now = Utc::now();
        let (url, seen) = mock(500, r#"{"error":"boom"}"#, false).await;
        let (mid, c) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());

        lane(&store, acct, Some(&url), cfg())
            .run(c.unwrap())
            .await
            .unwrap();

        assert_eq!(
            ledger(&store, acct, mid).unwrap().decision,
            NotifyDecision::Unavailable
        );
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "one attempt, no backoff: a retry loop here would sleep away the \
             window the lane exists for"
        );
        assert!(store.events_after(acct, 0, 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_model_that_never_answers_gives_up_inside_the_timeout() {
        let (store, acct) = store();
        let now = Utc::now();
        let (url, _) = mock(200, "{}", /* hang */ true).await;
        let (mid, c) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());

        let started = Instant::now();
        lane(&store, acct, Some(&url), cfg())
            .run(c.unwrap())
            .await
            .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(
            ledger(&store, acct, mid).unwrap().decision,
            NotifyDecision::Unavailable
        );
        // BOUNDED, not slept: the assertion is that the deadline FIRED, and the
        // ceiling is generous enough that a loaded CI box cannot fail it.
        assert!(
            elapsed >= Duration::from_secs(1) && elapsed < Duration::from_secs(6),
            "gave up after {elapsed:?}, not at the 1s deadline"
        );
    }

    #[test]
    fn a_poisoned_in_flight_set_still_releases_its_claim() {
        let set: Arc<Mutex<std::collections::HashSet<i64>>> =
            Arc::new(Mutex::new(std::collections::HashSet::new()));
        let victim = set.clone();
        let _ = std::thread::spawn(move || {
            let _held = victim.lock().unwrap();
            panic!("another task died holding the guard");
        })
        .join();
        assert!(set.is_poisoned(), "the set is poisoned but intact");

        {
            let _claim = InFlight::claim(&set, 7).expect("a claim over a poisoned but intact set");
            assert!(InFlight::claim(&set, 7).is_none(), "and it excludes");
        }
        assert!(
            InFlight::claim(&set, 7).is_some(),
            "the claim was released: a message must not go dark for the life of \
             the process because some unrelated task panicked"
        );
    }

    #[tokio::test]
    async fn an_unstamped_row_is_not_a_candidate_and_leaves_no_ledger_row() {
        let (store, acct) = store();
        let now = Utc::now();
        let f = RawFetched {
            account_id: acct,
            gmail_msg_id: "g-back".to_string(),
            gmail_thread_id: None,
            raw: note_eml(now).into_bytes(),
            internal_date: Some(now),
            is_sent: false,
            is_spam: false,
            account_addr: "me@example.com".to_string(),
        };
        let rules = store.list_sender_rules(acct).unwrap();
        let mut triaged = ingest_with_rules(&f, &Stage1Config::default(), now, &rules, |_| false);
        triaged.notify_eligible_at = super::super::notify_eligible_stamp(
            &triaged,
            super::super::IngestOrigin::Backfill,
            &cfg(),
            now,
        );
        let mid = store.ingest_message(&triaged).unwrap();
        assert_eq!(triaged.notify_eligible_at, None);

        let c = candidate(&triaged, mid, &rules, &cfg(), |_| false);
        assert!(c.is_none());
        assert!(ledger(&store, acct, mid).is_none());
    }

    #[tokio::test]
    async fn a_401_parks_the_lane_and_refunds_the_charge() {
        let (store, acct) = store();
        let now = Utc::now();
        let (url, seen) = mock(
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"x"}}"#,
            false,
        )
        .await;
        let lane = lane(&store, acct, Some(&url), cfg());
        let day = Utc::now().format("%Y-%m-%d").to_string();

        let (mid1, c1) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());
        lane.clone().run(c1.unwrap()).await.unwrap();
        assert_eq!(
            ledger(&store, acct, mid1).unwrap().decision,
            NotifyDecision::Unavailable
        );
        assert_eq!(
            store
                .stage2_budget_used(acct, NOTIFY_FAST_BUDGET_KEY, &day)
                .unwrap(),
            0,
            "a 4xx in ~0ms spends no tokens; leaving it charged would outlive \
             the outage by hours, because the budget key is the UTC day"
        );

        let second = note_eml(now).replace("dana@elsewhere.example", "sam@elsewhere.example");
        let (mid2, c2) = ingest(&store, acct, "g2", &second, now, &cfg());
        lane.run(c2.unwrap()).await.unwrap();
        assert_eq!(
            ledger(&store, acct, mid2).unwrap().decision,
            NotifyDecision::Unavailable
        );
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the lane is parked: the second message cost no request"
        );
        assert!(store.events_after(acct, 0, 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_qualified_model_id_goes_on_the_wire_and_not_just_in_the_ledger() {
        let (store, acct) = store();
        let now = Utc::now();
        // A loopback mock IS a gateway to `is_gateway_url`, which is "anything
        // that is not api.anthropic.com".
        let (url, seen) = mock(200, verdict(80), false).await;
        let (mid, c) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());

        lane(&store, acct, Some(&url), cfg())
            .run(c.unwrap())
            .await
            .unwrap();

        let req = seen.lock().unwrap().first().cloned().expect("one request");
        let body = req.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
        let sent: serde_json::Value = serde_json::from_str(&body).expect("a JSON request body");
        assert_eq!(
            sent["model"].as_str(),
            Some("anthropic/claude-haiku-4-5"),
            "the gateway routes on the prefix; a bare id 400s before anything \
             else is consulted"
        );
        // ONE STRING, so the ledger cannot describe a call that was never made.
        assert_eq!(
            ledger(&store, acct, mid).unwrap().model_used.as_deref(),
            sent["model"].as_str()
        );

        // AND THE OTHER DIRECTION: the direct Anthropic API refuses the prefix,
        // so `new` must leave the configured id alone there. Asserted on the
        // config the request is built from, which is the same field
        // `notify_llm::classify_at` reads.
        let direct = lane(&store, acct, Some(llm::API_URL), cfg());
        assert_eq!(direct.cfg.model, NotifyConfig::default().model);
    }

    #[tokio::test]
    async fn a_re_ingested_message_is_never_decided_twice() {
        let (store, acct) = store();
        let now = Utc::now();
        let (url, seen) = mock(200, verdict(80), false).await;
        let day = now.format("%Y-%m-%d").to_string();
        let lane = lane(&store, acct, Some(&url), cfg());

        let (mid, c1) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());
        lane.clone()
            .run(c1.expect("a model candidate"))
            .await
            .unwrap();

        // THE SAME gmail id THROUGH THE SAME PIPELINE, which is what a re-walk
        // is: `UNIQUE(account_id, gmail_msg_id)` collapses the message row and
        // `notify_eligible_stamp` computes a fresh `Some` off a `Date:` that is
        // still minutes old.
        let (mid2, c2) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());
        assert_eq!(mid2, mid, "a re-ingest is the same row");
        lane.clone()
            .run(c2.expect("still a candidate: the stamp recomputes"))
            .await
            .unwrap();

        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the second run must not pay for a second model call"
        );
        assert_eq!(
            store
                .stage2_budget_used(acct, NOTIFY_FAST_BUDGET_KEY, &day)
                .unwrap(),
            1,
            "nor charge a second daily-cap unit"
        );
        let rows = store
            .notify_decisions_since(acct, now - chrono::Duration::hours(1), 100)
            .unwrap();
        assert_eq!(rows.len(), 1, "one decision per (message, lane), forever");
        assert_eq!(rows[0].decision, NotifyDecision::Sent);
        assert_eq!(store.events_after(acct, 0, 10).unwrap().len(), 1);
        // THE METRIC IS THE LEDGER'S SHADOW, NEVER A SECOND BOOKKEEPING: the
        // counter hangs off the insert's `Ok(true)`, so an IGNORED duplicate
        // cannot make `squelchd_notify_decisions_total{lane="fast"}` claim a
        // decision `SELECT lane, decision, count(*) FROM notify_decisions` does
        // not hold — and §11.11 reads that query beside the graph.
        let text = crate::metrics::render(&lane.metrics, None);
        assert!(
            text.contains("squelchd_notify_decisions_total{lane=\"fast\",decision=\"sent\"} 1\n"),
            "one row, one count"
        );
    }

    #[tokio::test]
    async fn two_overlapping_runs_decide_once() {
        let (store, acct) = store();
        let now = Utc::now();
        let day = now.format("%Y-%m-%d").to_string();
        let (release, rx) = tokio::sync::watch::channel(false);
        let (url, seen) = mock_held(200, verdict(80), Hold::Until(rx)).await;
        let (mid, c1) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());
        let lane = lane(&store, acct, Some(&url), cfg());

        let first = tokio::spawn(lane.clone().run(c1.expect("a model candidate")));
        let landed = seen.clone();
        until("the first call never went out", || {
            landed.lock().unwrap().len() == 1
        })
        .await;

        // THE SECOND TICK, with the first call still outstanding and no ledger
        // row written yet. It must return without a call, a charge or a row.
        let (mid2, c2) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());
        assert_eq!(mid2, mid, "a re-ingest is the same row");
        lane.clone()
            .run(c2.expect("still a candidate"))
            .await
            .unwrap();
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the second run must not pay for a second model call while the \
             first is still out"
        );

        release.send(true).unwrap();
        first.await.unwrap().unwrap();

        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(
            store
                .stage2_budget_used(acct, NOTIFY_FAST_BUDGET_KEY, &day)
                .unwrap(),
            1,
            "nor charge a second daily-cap unit"
        );
        let rows = store
            .notify_decisions_since(acct, now - chrono::Duration::hours(1), 100)
            .unwrap();
        assert_eq!(rows.len(), 1, "one decision per (message, lane), forever");
        assert_eq!(rows[0].decision, NotifyDecision::Sent);
        assert_eq!(store.events_after(acct, 0, 10).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_openai_daemon_puts_the_bare_model_id_on_the_wire() {
        let (store, acct) = store();
        let now = Utc::now();
        let (url, seen) = mock(200, "{}", false).await;
        let (_, c) = ingest(&store, acct, "g1", &note_eml(now), now, &cfg());
        let lane = Arc::new(NotifyLane::new(
            store.clone(),
            reqwest::Client::new(),
            cfg(),
            Config::default().stage1.known_contact_importance,
            Some(ResolvedLlm {
                api_key: "sk-test".to_string(),
                provider: Stage2Provider::OpenAI,
                url: url.clone(),
            }),
            SyncMetrics::new(),
            acct,
            Arc::new(std::sync::Mutex::new(super::super::WarnDays::default())),
        ));
        assert_eq!(
            lane.cfg.model,
            NotifyConfig::default().model,
            "no `anthropic/` on an OpenAI deployment"
        );

        lane.run(c.expect("a model candidate")).await.unwrap();

        let req = seen.lock().unwrap().first().cloned().expect("one request");
        let body = req.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
        let sent: serde_json::Value = serde_json::from_str(&body).expect("a JSON request body");
        assert_eq!(
            sent["model"].as_str(),
            Some(NotifyConfig::default().model.as_str()),
            "OpenAI has never heard of `anthropic/claude-haiku-4-5` and answers \
             400, which parks the lane forever"
        );
    }

    #[tokio::test]
    async fn unavailable_model_never_falls_back_to_a_seed() {
        let (store, acct) = store();
        let now = Utc::now();
        let (id, candidate) = ingest(&store, acct, "no-model", &note_eml(now), now, &cfg());
        lane(&store, acct, None, cfg())
            .run(candidate.unwrap())
            .await
            .unwrap();
        assert_eq!(
            ledger(&store, acct, id).unwrap().decision,
            NotifyDecision::Unavailable
        );
        assert!(store.events_after(acct, 0, 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn model_auth_pushes_even_at_zero_importance() {
        let (store, acct) = store();
        let now = Utc::now();
        let response = verdict(0);
        // Build the auth reply structurally, without coupling the test to escaping.
        let mut response: serde_json::Value = serde_json::from_str(&response).unwrap();
        let mut assessment: serde_json::Value =
            serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
        assessment["is_auth"] = serde_json::json!(true);
        assessment["reason"] =
            serde_json::json!("Account login alert requires immediate awareness.");
        response["content"][0]["text"] = serde_json::json!(assessment.to_string());
        let (url, seen) = mock(200, response.to_string(), false).await;
        let (id, candidate) = ingest(&store, acct, "auth", &note_eml(now), now, &cfg());
        lane(&store, acct, Some(&url), cfg())
            .run(candidate.unwrap())
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(
            ledger(&store, acct, id).unwrap().decision,
            NotifyDecision::Sent
        );
        let events = store.events_after(acct, 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, crate::types::EventKind::Urgent);
        assert_eq!(events[0].message_id, id);
        assert!(
            events[0].is_auth,
            "login alerts retain their model auth flag"
        );
        assert_eq!(events[0].sealed_kind, None);
        let stored = store
            .latest_notification_assessment(acct, id, LaneLabel::Fast)
            .unwrap()
            .unwrap();
        assert!(stored.is_auth);
        assert_eq!(stored.importance, 0);
        assert_eq!(
            stored.reason,
            "Account login alert requires immediate awareness."
        );
        assert_eq!(stored.prompt_version, notify_llm::PROMPT_VERSION);
        assert!(
            store
                .latest_notification_assessment(acct + 1, id, LaneLabel::Fast)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .latest_notification_assessment(acct, id, LaneLabel::Deliberate)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn known_contacts_do_not_have_a_score_floor() {
        let (store, acct) = store();
        let now = Utc::now();
        let (url, _) = mock(200, verdict(0), false).await;
        let (id, candidate) = ingest(&store, acct, "contact", &note_eml(now), now, &cfg());
        let mut candidate = candidate.unwrap();
        candidate.is_known_contact = true;
        lane(&store, acct, Some(&url), cfg())
            .run(candidate)
            .await
            .unwrap();
        let row = ledger(&store, acct, id).unwrap();
        assert_eq!(row.decision, NotifyDecision::DeclinedByModel);
        assert_eq!(row.notify_importance, Some(0));
    }

    #[tokio::test]
    async fn sender_preferences_are_model_context_not_a_delivery_veto() {
        let (store, acct) = store();
        let now = Utc::now();
        let (url, seen) = mock(200, verdict(90), false).await;
        let (id, candidate) = ingest(&store, acct, "preference", &note_eml(now), now, &cfg());
        let mut candidate = candidate.unwrap();
        candidate.sender_preferences =
            Some("Squelch this sender unless a service is cancelled".into());
        lane(&store, acct, Some(&url), cfg())
            .run(candidate)
            .await
            .unwrap();
        assert_eq!(
            ledger(&store, acct, id).unwrap().decision,
            NotifyDecision::Sent
        );
        assert!(
            seen.lock().unwrap()[0].contains("Squelch this sender unless a service is cancelled")
        );
    }

    fn durable_context(
        store: &SqliteStore,
        account: AccountId,
        message: i64,
    ) -> crate::store::agent_triage::AgentContext {
        use crate::store::agent_triage::AgentTriageStore;
        store
            .enqueue_agent_triage(account, message, "notification-test", false)
            .unwrap();
        let job = store
            .claim_agent_job(account, "triage", Utc::now(), 120)
            .unwrap()
            .unwrap();
        assert_eq!(job.message_id, message);
        store.load_agent_context(&job).unwrap()
    }

    #[test]
    fn durable_candidate_uses_contact_and_only_matching_rule_context() {
        let (store, acct) = store();
        let now = Utc::now();
        let (id, _) = ingest(
            &store,
            acct,
            "candidate-context",
            &note_eml(now),
            now,
            &cfg(),
        );
        let mut context = durable_context(&store, acct, id);
        context.sender_is_contact = true;
        context.rules = vec![serde_json::json!({"want_text":"unrelated rule"})];
        context.matched_rules = vec![serde_json::json!({"want_text":"matching preference"})];
        let candidate = candidate_from_context(&context, &cfg()).unwrap();
        assert!(candidate.is_known_contact);
        let preferences = candidate.sender_preferences.unwrap();
        assert!(preferences.contains("matching preference"));
        assert!(!preferences.contains("unrelated rule"));
        context.matched_rules.clear();
        assert!(
            candidate_from_context(&context, &cfg())
                .unwrap()
                .sender_preferences
                .is_none()
        );
    }

    fn advice() -> crate::triage::decision::NotificationAdvice {
        crate::triage::decision::NotificationAdvice {
            importance: 90,
            title: "Service update".into(),
            body: "A cancellation needs your attention".into(),
            reason: "A real consequence".into(),
        }
    }

    #[tokio::test]
    async fn full_agent_can_rescue_fast_decline_and_delivery_stays_unique() {
        let (store, account) = store();
        let now = Utc::now();
        let (url, _) = mock(200, verdict(10), false).await;
        let (id, candidate) = ingest(&store, account, "rescue", &note_eml(now), now, &cfg());
        let lane = lane(&store, account, Some(&url), cfg());
        lane.clone().run(candidate.unwrap()).await.unwrap();
        assert!(store.events_after(account, 0, 10).unwrap().is_empty());
        let context = durable_context(&store, account, id);
        lane.request_assessed(&context, false, &advice(), "full-model")
            .unwrap();
        lane.request_assessed(&context, false, &advice(), "full-model")
            .unwrap();
        assert_eq!(store.events_after(account, 0, 10).unwrap().len(), 1);
        let rows = store
            .notify_decisions_since(account, now - chrono::Duration::seconds(1), 10)
            .unwrap();
        assert!(rows.iter().any(|r| r.lane == LaneLabel::Fast && r.decision == NotifyDecision::DeclinedByModel));
        assert!(
            rows.iter()
                .any(|r| r.lane == LaneLabel::Deliberate && r.decision == NotifyDecision::Sent)
        );
    }

    #[tokio::test]
    async fn full_agent_first_and_fast_agent_second_share_the_arrival_key() {
        let (store, account) = store();
        let now = Utc::now();
        let (url, _) = mock(200, verdict(90), false).await;
        let (id, candidate) = ingest(&store, account, "full-first", &note_eml(now), now, &cfg());
        let lane = lane(&store, account, Some(&url), cfg());
        let context = durable_context(&store, account, id);
        let mut auth_advice = advice();
        auth_advice.importance = 0;
        lane.request_assessed(&context, true, &auth_advice, "full-model")
            .unwrap();
        lane.clone().run(candidate.unwrap()).await.unwrap();
        let events = store.events_after(account, 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].is_auth,
            "deliberate auth survives dedup against later non-auth fast assessment"
        );
        let rows = store
            .notify_decisions_since(account, now - chrono::Duration::seconds(1), 10)
            .unwrap();
        assert!(
            rows.iter()
                .any(|r| r.lane == LaneLabel::Fast && r.decision == NotifyDecision::WouldSend)
        );
    }

    #[test]
    fn opening_message_before_full_assessment_prevents_a_late_push() {
        use crate::store::agent_triage::AgentTriageStore;
        let (store, account) = store();
        let now = Utc::now();
        let (id, _) = ingest(&store, account, "opened", &note_eml(now), now, &cfg());
        let context = durable_context(&store, account, id);
        store.acknowledge_agent_message(account, id, now).unwrap();
        lane(&store, account, None, cfg())
            .request_assessed(&context, true, &advice(), "full-model")
            .unwrap();
        assert!(store.events_after(account, 0, 10).unwrap().is_empty());
    }

    #[test]
    fn store_failures_propagate_and_retry_does_not_duplicate_delivery() {
        let dir = std::env::temp_dir().join(format!(
            "notify-failure-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mail.db");
        let store = Arc::new(SqliteStore::open(&path).unwrap());
        let account = store.ensure_account("me@example.com").unwrap();
        let now = Utc::now();
        let (message, _) = ingest(&store, account, "fault", &note_eml(now), now, &cfg());
        let context = durable_context(&store, account, message);
        let lane = lane(&store, account, None, cfg());
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON events BEGIN SELECT RAISE(FAIL,'injected failure'); END;").unwrap();
        assert!(
            lane.request_assessed(&context, false, &advice(), "full-model")
                .is_err()
        );
        assert!(store.events_after(account, 0, 10).unwrap().is_empty());
        assert!(ledger(&store, account, message).is_none());
        conn.execute_batch("DROP TRIGGER fail_event; CREATE TRIGGER fail_ledger BEFORE INSERT ON notify_decisions BEGIN SELECT RAISE(FAIL,'injected ledger failure'); END;").unwrap();
        assert!(
            lane.request_assessed(&context, false, &advice(), "full-model")
                .is_err()
        );
        assert_eq!(store.events_after(account, 0, 10).unwrap().len(), 1);
        conn.execute_batch("DROP TRIGGER fail_ledger;").unwrap();
        lane.request_assessed(&context, false, &advice(), "full-model")
            .unwrap();
        assert_eq!(store.events_after(account, 0, 10).unwrap().len(), 1);
        assert_eq!(
            ledger(&store, account, message).unwrap().decision,
            NotifyDecision::WouldSend
        );
        drop(conn);
        drop(lane);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
