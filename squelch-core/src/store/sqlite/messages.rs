//! Message ingest, thread views, attachments, deadlines, sync cursors
//! and the sealed-mail reads.

use super::specialists::{upsert_calendar_conn, upsert_receipt_conn, upsert_shipment_conn};
use super::*;

/// Apply the unsubscribe VIOLATION bump for a just-stored inbound message, in
/// the caller's transaction: an unresolved `unsubscribes` row for
/// `(account_id, lower(from_addr))` whose request is more than 72h older than
/// this `received_at` gets `violation_count + 1` and `last_violation_at`. No
/// row, already resolved, or still within grace is a silent no-op.
fn bump_unsub_violation_conn(
    conn: &Connection,
    account_id: AccountId,
    from_addr: &str,
    received_at: DateTime<Utc>,
) -> Result<()> {
    let sender = from_addr.trim().to_ascii_lowercase();
    if sender.is_empty() {
        return Ok(());
    }
    // Read the outstanding request so the grace comparison runs on real
    // timestamps in Rust rather than as lexical string math in SQL.
    let row: Option<String> = conn
        .query_row(
            "SELECT requested_at FROM unsubscribes
             WHERE account_id = ?1 AND sender_addr = ?2 AND resolution IS NULL",
            params![account_id, sender],
            |r| r.get(0),
        )
        .optional()?;
    let Some(requested_s) = row else {
        return Ok(());
    };
    let requested_at = parse_dt(&requested_s)?;
    if received_at > requested_at + chrono::Duration::hours(72) {
        conn.execute(
            "UPDATE unsubscribes
             SET violation_count = violation_count + 1, last_violation_at = ?3
             WHERE account_id = ?1 AND sender_addr = ?2 AND resolution IS NULL",
            params![account_id, sender, received_at.to_rfc3339()],
        )?;
    }
    Ok(())
}

/// Upsert a message + FTS + Sent-derived contacts against an explicit
/// connection/transaction handle. Shared by [`SqliteStore::upsert_message`] and
/// the transactional [`Store::ingest_message`] path so both stay in sync.
///
/// `is_sent` is STICKY TO 0 on conflict (`MIN`): a row ever seen as received
/// can never flip to sent. `is_sent=1` removes a message from every listing
/// surface, so the flip would vanish inbound mail on the strength of a single
/// mislabeled sighting — and Gmail's label-filtered history has served inbound
/// mail through a SENT walk in production. The sync engine's walk ordering is
/// the first defense; this clause is the backstop that holds regardless of
/// ingest order or upstream filter quality. The reverse flip (1 -> 0) stays
/// allowed: a message can only gain visibility, never lose it.
///
/// `is_spam` is STICKY TO 0 the same way and for the same shape of reason: a
/// message ever seen outside the spam label must not be hidden by a later
/// sighting inside it. The SPAM walk runs last and subtracts what the other two
/// already returned, so the clause is a backstop rather than the mechanism.
///
/// `to_addrs` is the one column that PREFERS THE STORED VALUE over a NULL
/// (`COALESCE(excluded, messages)`): only a sent-path ingest parses recipients,
/// so a re-fetch that skips them — or an old row the backfill already filled —
/// must not be blanked by the next writer that has no opinion.
fn upsert_message_conn(conn: &Connection, msg: &NewMessage) -> Result<i64> {
    conn.execute(
        "INSERT INTO messages(account_id, gmail_msg_id, thread_id, from_addr, from_name,
             subject, received_at, snippet, body, body_html, is_sent, to_addrs,
             list_unsubscribe, list_unsub_one_click, auth_pass, is_spam)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
         ON CONFLICT(account_id, gmail_msg_id) DO UPDATE SET
             thread_id=excluded.thread_id, from_addr=excluded.from_addr,
             from_name=excluded.from_name, subject=excluded.subject,
             received_at=excluded.received_at, snippet=excluded.snippet,
             body=excluded.body, body_html=excluded.body_html,
             is_sent=MIN(messages.is_sent, excluded.is_sent),
             to_addrs=COALESCE(excluded.to_addrs, messages.to_addrs),
             list_unsubscribe=excluded.list_unsubscribe,
             list_unsub_one_click=excluded.list_unsub_one_click,
             auth_pass=excluded.auth_pass,
             is_spam=MIN(messages.is_spam, excluded.is_spam)",
        params![
            msg.account_id,
            msg.gmail_msg_id,
            msg.thread_id,
            msg.from_addr,
            msg.from_name,
            msg.subject,
            msg.received_at.to_rfc3339(),
            msg.snippet,
            msg.body,
            msg.body_html,
            msg.is_sent as i64,
            msg.to_addrs,
            msg.list_unsubscribe,
            msg.list_unsub_one_click as i64,
            msg.auth_pass.map(|p| p as i64),
            msg.is_spam as i64,
        ],
    )?;
    let id: i64 = conn.query_row(
        "SELECT id FROM messages WHERE account_id=?1 AND gmail_msg_id=?2",
        params![msg.account_id, msg.gmail_msg_id],
        |r| r.get(0),
    )?;

    // Keep the FTS index in sync.
    conn.execute("DELETE FROM messages_fts WHERE rowid=?1", params![id])?;
    conn.execute(
        "INSERT INTO messages_fts(rowid, subject, body) VALUES(?1,?2,?3)",
        params![id, msg.subject, msg.body],
    )?;

    // The sender directory, for the search field's `from:` menu. Inbound and
    // non-spam only: sent mail's From is the user, and spam is a structural
    // exclusion everywhere else too. Judged on THIS sighting's flags rather
    // than the row's sticky ones on purpose: a message first seen as spam and
    // now seen outside the label has just become visible, and this is the
    // sighting that should register its sender (the count is recomputed from
    // the row, so the sticky flags still decide what it says).
    if !msg.is_sent && !msg.is_spam {
        super::senders::bump_sender_conn(
            conn,
            msg.account_id,
            &msg.from_addr,
            msg.from_name.as_deref(),
            &msg.received_at.to_rfc3339(),
        )?;
    }

    // Contacts are NOT seeded here: Sent mail's From header is the user's own
    // address. They come from the To/Cc recipients in `ingest_message`.
    Ok(id)
}

/// Seed the contacts table from a Sent message's recipients, each bumping its
/// `sent_count`. Addresses arrive de-duplicated and stripped of the account's own
/// address, so only empties are skipped. Received mail passes an empty list.
fn seed_contacts_conn(
    conn: &Connection,
    account_id: AccountId,
    recipients: &[String],
    first_seen: &str,
) -> Result<()> {
    for addr in recipients {
        if addr.trim().is_empty() {
            continue;
        }
        conn.execute(
            "INSERT INTO contacts(account_id, addr, sent_count, first_seen, last_sent_at)
             VALUES(?1,?2,1,?3,?3)
             ON CONFLICT(account_id, addr) DO UPDATE SET
                 sent_count = sent_count + 1,
                 last_sent_at = MAX(COALESCE(last_sent_at,''), ?3)",
            params![account_id, addr, first_seen],
        )?;
    }
    Ok(())
}

/// Replace this message's attachment rows, in the caller's transaction.
/// DELETE-then-INSERT keeps re-ingest idempotent; `data == None` writes a NULL
/// blob (over-cap, metadata only) while `size_bytes` stays the real decoded size.
/// Written for sealed mail too — the byte-serving path guards sealed parents.
///
/// INSERT OR IGNORE, not plain INSERT: a sender can attach the same file twice,
/// and a UNIQUE(account,message,filename,size) violation would roll back the
/// ENTIRE message ingest — a remote ingest DoS. Collapsing identical duplicates
/// to one row is the wanted outcome.
fn insert_attachments_conn(
    conn: &Connection,
    account_id: AccountId,
    message_id: i64,
    attachments: &[AttachmentInfo],
) -> Result<()> {
    conn.execute(
        "DELETE FROM attachments WHERE account_id=?1 AND message_id=?2",
        params![account_id, message_id],
    )?;
    for a in attachments {
        conn.execute(
            "INSERT OR IGNORE INTO attachments(account_id, message_id, filename, mime, size_bytes, data, content_id)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                account_id,
                message_id,
                a.filename,
                a.mime,
                a.size_bytes,
                a.data.as_deref(),
                a.content_id,
            ],
        )?;
    }
    Ok(())
}

/// Check the current assessment and every consumed source. Unknown assessments
/// and changed sources fail closed, including dependencies of derived summaries.
pub(super) fn external_message_allowed_conn(
    conn: &Connection,
    account_id: AccountId,
    message_id: i64,
) -> Result<bool> {
    Ok(conn.query_row(
        "WITH RECURSIVE dependencies(message_id) AS (
             SELECT ?2
             UNION
             SELECT d.source_message_id FROM agent_decision_sources d
             JOIN dependencies p ON p.message_id=d.message_id
             WHERE d.account_id=?1
         )
         SELECT EXISTS(SELECT 1 FROM messages WHERE account_id=?1 AND id=?2)
           AND NOT EXISTS(
             SELECT 1 FROM dependencies d
             LEFT JOIN messages m ON m.id=d.message_id AND m.account_id=?1
             LEFT JOIN agent_message_state a ON a.message_id=d.message_id AND a.account_id=?1
             WHERE m.id IS NULL OR a.access IS NULL OR a.access!='allowed'
               OR EXISTS(SELECT 1 FROM agent_triage_corrections c WHERE c.account_id=?1
                   AND c.message_id=d.message_id AND c.field='external_access' AND c.value_json='true')
           )
           AND NOT EXISTS(
             SELECT 1 FROM agent_decision_sources d
             JOIN dependencies p ON p.message_id=d.message_id
             LEFT JOIN agent_message_state a ON a.message_id=d.source_message_id AND a.account_id=?1
             WHERE d.account_id=?1 AND (a.revision IS NULL OR a.revision!=d.source_revision)
           )",
        params![account_id, message_id],
        |row| row.get(0),
    )?)
}

/// External full-thread reads require every sibling to be allowed. The human
/// reader bypasses this guard and may open pending or restricted mail.
pub(super) fn thread_guard_and_subject(
    conn: &Connection,
    account_id: AccountId,
    thread_id: &str,
) -> Result<String> {
    let mut stmt = conn.prepare("SELECT id FROM messages WHERE account_id=?1 AND thread_id=?2")?;
    let ids = stmt
        .query_map(params![account_id, thread_id], |row| row.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err(CoreError::NotFound);
    }
    for id in ids {
        if !external_message_allowed_conn(conn, account_id, id)? {
            return Err(CoreError::NotFound);
        }
    }
    conn.query_row(THREAD_SUBJECT_SQL, params![account_id, thread_id], |row| {
        row.get(0)
    })
    .optional()?
    .ok_or(CoreError::NotFound)
}

/// Only an explicit external thread-open request may schedule source assessments.
/// Permission probes for human cache policy, listings and search remain pure.
fn request_external_thread_access_conn(
    conn: &Connection,
    account_id: AccountId,
    thread_id: &str,
) -> Result<()> {
    // Reading a legacy thread is demand for assessing its unseen sources. Queue
    // a small batch, never every ancestor at arrival. Existing assessments and
    // active or terminal work remain untouched; access stays closed until every source passes.
    let missing: Vec<i64> = conn
        .prepare(
            "SELECT m.id FROM messages m
         LEFT JOIN agent_message_state a ON a.account_id=m.account_id AND a.message_id=m.id
         WHERE m.account_id=?1 AND m.thread_id=?2
           AND (a.access IS NULL OR a.access='pending')
           AND NOT EXISTS(SELECT 1 FROM agent_triage_jobs j
             WHERE j.account_id=m.account_id AND j.message_id=m.id
               AND j.kind IN ('triage','access') AND j.input_revision=a.revision)
         ORDER BY m.received_at DESC,m.id DESC LIMIT 8",
        )?
        .query_map(params![account_id, thread_id], |row| row.get(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    super::agent_triage::ensure_agent_source_ids_conn(conn, account_id, &missing)?;
    Ok(())
}

// Shared with the query-plan regression test: LIMIT 1 must seek the thread,
// not walk the account's received_at index looking for its first message.
pub(super) const THREAD_SUBJECT_SQL: &str = "SELECT subject FROM messages
    WHERE account_id=?1 AND thread_id=?2
    ORDER BY received_at ASC LIMIT 1";

/// Replace this message's `deadlines` row, in the caller's transaction:
/// DELETE-then-INSERT so a re-apply/re-ingest is idempotent, and `None` simply
/// leaves the message dateless. Shared by ingest and both stage applies, whose
/// callers own the sealed guard (a sealed message must never grow a deadline).
pub(super) fn rewrite_deadline_conn(
    conn: &Connection,
    account_id: AccountId,
    message_id: i64,
    deadline: Option<&crate::triage::DeadlineHit>,
) -> Result<()> {
    conn.execute(
        "DELETE FROM deadlines WHERE message_id=?1",
        params![message_id],
    )?;
    if let Some(d) = deadline {
        conn.execute(
            "INSERT INTO deadlines(account_id, message_id, kind, amount, currency,
                 due_at, past_due, source)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                account_id,
                message_id,
                d.kind,
                d.amount,
                d.currency,
                d.due_at.to_rfc3339(),
                d.past_due as i64,
                d.source,
            ],
        )?;
    }
    Ok(())
}

/// Load one message's attachment metadata (NO bytes) as the human-door wire
/// shape, ordered by row id; `downloadable` is `data IS NOT NULL`. The CALLER
/// owns the sealed guard.
fn load_client_attachments_conn(
    conn: &Connection,
    account_id: AccountId,
    message_id: i64,
) -> Result<Vec<ClientAttachment>> {
    let mut stmt = conn.prepare(
        "SELECT id, filename, mime, size_bytes, data IS NOT NULL, content_id
         FROM attachments
         WHERE account_id=?1 AND message_id=?2
         ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map(params![account_id, message_id], |r| {
            Ok(ClientAttachment {
                id: r.get(0)?,
                filename: r.get(1)?,
                mime: r.get(2)?,
                size: r.get(3)?,
                downloadable: r.get(4)?,
                content_id: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(super) fn external_shipment_allowed_conn(
    conn: &Connection,
    account_id: AccountId,
    shipment_id: i64,
) -> Result<bool> {
    type ShipmentProvenance = (Option<i64>, Option<i64>, Option<i64>, String);
    let provenance: Option<ShipmentProvenance> = conn
        .query_row(
            "SELECT created_by_message_id,last_message_id,item_name_msg,item_name
             FROM shipments WHERE account_id=?1 AND id=?2",
            params![account_id, shipment_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((Some(created), Some(last), name_source, name)) = provenance else {
        return Ok(false);
    };
    if !name.trim().is_empty() && name_source.is_none() {
        return Ok(false);
    }
    for id in [Some(created), Some(last), name_source]
        .into_iter()
        .flatten()
    {
        if !external_message_allowed_conn(conn, account_id, id)? {
            return Ok(false);
        }
        let thread: String = conn.query_row(
            "SELECT thread_id FROM messages WHERE account_id=?1 AND id=?2",
            params![account_id, id],
            |r| r.get(0),
        )?;
        if thread_guard_and_subject(conn, account_id, &thread).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

impl SqliteStore {
    pub(super) fn upsert_message(&self, msg: &NewMessage) -> Result<i64> {
        let conn = self.lock()?;
        upsert_message_conn(&conn, msg)
    }

    /// Clear provider spam after the caller restores the message in Gmail.
    /// Reset human attention state and enqueue canonical triage in the same
    /// transaction. Rescue is push-silent, including for older messages. Legacy
    /// pass markers are cleared for compatibility with older diagnostic readers.
    /// Returns false for an unknown, foreign-account, or already rescued message.
    pub(super) fn clear_spam(&self, account_id: AccountId, message_id: i64) -> Result<bool> {
        let mut conn = self.lock()?;
        let now = Utc::now().to_rfc3339();
        let tx = conn.transaction()?;
        let cleared = tx.execute(
            "UPDATE messages SET is_spam = 0
             WHERE account_id = ?1 AND id = ?2 AND is_spam = 1",
            params![account_id, message_id],
        )?;
        if cleared == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE triage
                SET stage1_model_used = NULL, model_used = NULL, needs_stage2 = 0,
                    extractor_model_used = NULL, retriage_at = ?3,
                    status = 'new', surfaced_at = NULL, resolved_at = NULL
              WHERE account_id = ?1 AND message_id = ?2",
            params![account_id, message_id, now],
        )?;
        // The row just became visible WITHOUT passing through the upsert, so
        // its sender registers here or not at all: a sender whose only mail
        // was misfiled as spam has no directory row until this moment.
        let (from_addr, from_name, received_at): (String, Option<String>, String) = tx.query_row(
            "SELECT from_addr, from_name, received_at FROM messages
                 WHERE account_id = ?1 AND id = ?2",
            params![account_id, message_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        super::senders::bump_sender_conn(
            &tx,
            account_id,
            &from_addr,
            from_name.as_deref(),
            &received_at,
        )?;
        super::agent_triage::enqueue_agent_triage_conn(
            &tx, account_id, message_id, "not_spam", false,
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Pure permission probe: human cache checks must never buy model work.
    pub fn external_thread_allowed(&self, account_id: AccountId, thread_id: &str) -> Result<bool> {
        let conn = self.lock()?;
        match thread_guard_and_subject(&conn, account_id, thread_id) {
            Ok(_) => Ok(true),
            Err(CoreError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// A shipment combines tracking, status, and name fields from different
    /// messages. Every recorded contributor must have a current allowed decision.
    /// Legacy records without provenance remain available only to the human.
    pub fn external_shipment_allowed(
        &self,
        account_id: AccountId,
        shipment_id: i64,
    ) -> Result<bool> {
        let conn = self.lock()?;
        external_shipment_allowed_conn(&conn, account_id, shipment_id)
    }

    pub(super) fn thread_view(&self, account_id: AccountId, thread_id: &str) -> Result<ThreadView> {
        let conn = self.lock()?;
        request_external_thread_access_conn(&conn, account_id, thread_id)?;
        let subject = thread_guard_and_subject(&conn, account_id, thread_id)?;

        // THE AGENT DOOR GETS NO SPAM AT ALL, not spam it is told to distrust.
        // Everything the agent reads is text it may act on, and provider spam is
        // text written to make a reader act; the human door can afford to show
        // it because a human is looking at a page that says who filed it. A
        // thread of nothing but spam therefore comes back empty here and 404s
        // below, which is the same shape sealed mail gets.
        let mut stmt = conn.prepare(
            "SELECT id, from_addr, from_name, received_at, body
             FROM messages
             WHERE account_id=?1 AND thread_id=?2 AND is_spam = 0
             ORDER BY received_at ASC",
        )?;
        let messages = stmt
            .query_map(params![account_id, thread_id], |r| {
                Ok(SanitizedMessage {
                    id: r.get(0)?,
                    from_addr: r.get(1)?,
                    from_name: r.get(2)?,
                    received_at: dt(r, 3)?,
                    content: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if messages.is_empty() {
            return Err(CoreError::NotFound);
        }

        Ok(ThreadView {
            thread_id: thread_id.to_string(),
            subject,
            messages,
        })
    }

    pub(super) fn thread_id_for_message(
        &self,
        account_id: AccountId,
        message_id: i64,
    ) -> Result<Option<String>> {
        let conn = self.lock()?;
        if !external_message_allowed_conn(&conn, account_id, message_id)? {
            return Ok(None);
        }
        let thread_id: Option<String> = conn
            .query_row(
                "SELECT m.thread_id
                 FROM messages m
                 WHERE m.account_id = ?1 AND m.id = ?2",
                params![account_id, message_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(thread_id)
    }

    pub(super) fn thread_view_with_html(
        &self,
        account_id: AccountId,
        thread_id: &str,
    ) -> Result<ClientThreadView> {
        let conn = self.lock()?;
        // The human can read pending and restricted mail without waiting for triage.
        let subject = conn
            .query_row(THREAD_SUBJECT_SQL, params![account_id, thread_id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or(CoreError::NotFound)?;

        // Per-message triage rides along for in-thread attention highlighting.
        // LEFT JOIN: a message somehow missing its triage row still renders,
        // just unhighlighted.
        //
        // `m.subject` rides along per message. The view's own `subject` is the
        // OLDEST message's (see `thread_guard_and_subject`), which titles the
        // conversation correctly and titles one message inside it WRONGLY the
        // moment somebody renames the thread — and a forward is composed from
        // one message, not from the conversation.
        //
        // The served `is_sent` is AUTHORSHIP, not the stored column: stored
        // `messages.is_sent` is a VISIBILITY flag that is sticky to 0 (see
        // `upsert_message_conn`) and the sync engine deliberately lets the INBOX
        // copy win for self-addressed mail, so a message the user wrote with
        // themselves on Cc — or mailed to themselves, or echoed back by a group
        // — stays pinned at 0. OR'ing the From address against the account's own
        // gives the reader the bit it actually aligns bubbles on. The accounts
        // LEFT JOIN is one row (`accounts.id` is the PK); LOWER on both sides
        // matches the ASCII case-folding every other address compare here uses,
        // and the empty-From guard keeps a blank sender from matching a blank
        // email, so a missing/NULL address falls back to the stored bit.
        let mut stmt = conn.prepare(
            "SELECT m.id, m.from_addr, m.from_name, m.received_at, m.body, m.body_html,
                    t.tier, t.deadline, t.status, t.one_line, m.auth_pass, m.subject,
                    m.is_spam,
                    (m.is_sent = 1
                     OR (TRIM(COALESCE(m.from_addr, '')) != ''
                         AND LOWER(TRIM(COALESCE(m.from_addr, ''))) =
                             LOWER(TRIM(COALESCE(a.email, ''))))) AS authored_by_account
             FROM messages m
             LEFT JOIN triage t ON t.message_id = m.id
             LEFT JOIN accounts a ON a.id = m.account_id
             WHERE m.account_id=?1 AND m.thread_id=?2
             ORDER BY m.received_at ASC",
        )?;
        // Collect first, releasing `stmt`'s borrow of `conn`, so the per-message
        // attachment query below can run on the same connection.
        let mut messages = stmt
            .query_map(params![account_id, thread_id], |r| {
                Ok(ClientMessage {
                    id: r.get(0)?,
                    from_addr: r.get(1)?,
                    from_name: r.get(2)?,
                    received_at: dt(r, 3)?,
                    subject: r.get(11)?,
                    content: r.get(4)?,
                    html: r.get(5)?,
                    attachments: Vec::new(), // filled below, once `stmt` is gone
                    // The computed authorship bit above: a boolean expression
                    // guarded against NULL on both sides, so every row answers.
                    is_sent: r.get::<_, i64>(13)? != 0,
                    is_spam: r.get::<_, i64>(12)? != 0,
                    tier: r
                        .get::<_, Option<String>>(6)?
                        .as_deref()
                        .and_then(Tier::parse),
                    deadline: dt_opt(r, 7)?,
                    attention_open: r.get::<_, Option<String>>(8)?.map(|s| s != "done"),
                    one_line: r.get::<_, Option<String>>(9)?.filter(|s| !s.is_empty()),
                    auth_pass: r.get::<_, Option<bool>>(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        for m in &mut messages {
            // ALWAYS present on the wire ([] when none). No sealed guard needed:
            // the whole view 404s any thread containing a sealed message.
            m.attachments = load_client_attachments_conn(&conn, account_id, m.id)?;
        }
        if messages.is_empty() {
            return Err(CoreError::NotFound);
        }

        Ok(ClientThreadView {
            thread_id: thread_id.to_string(),
            subject,
            messages,
        })
    }

    pub(super) fn attachment_bytes(
        &self,
        account_id: AccountId,
        attachment_id: i64,
    ) -> Result<Option<AttachmentBytes>> {
        let conn = self.lock()?;
        // SECURITY: the parent message must be non-sealed (a missing triage row
        // COALESCEs to 'normal'), so a sealed parent yields no row and the caller
        // 404s, indistinguishable from an unknown id. An existing row with NULL
        // `data` (over the ingest cap) flows out as `Some((.., None))` => 410.
        let row = conn
            .query_row(
                "SELECT a.filename, a.mime, a.data
                 FROM attachments a
                 JOIN messages m ON m.id = a.message_id AND m.account_id = a.account_id
                 LEFT JOIN triage t ON t.message_id = a.message_id
                 WHERE a.account_id = ?1 AND a.id = ?2",
                params![account_id, attachment_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<Vec<u8>>>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(row)
    }

    pub(super) fn deadlines(
        &self,
        account_id: AccountId,
        within_days: Option<u32>,
    ) -> Result<Vec<Deadline>> {
        let conn = self.lock()?;
        // Human deadline reads include restricted source mail; external callers check access.
        // within_days = None means "all".
        let cutoff =
            within_days.map(|d| (Utc::now() + chrono::Duration::days(d as i64)).to_rfc3339());
        let cutoff_ref: &dyn rusqlite::ToSql = match &cutoff {
            Some(s) => s,
            None => &"9999-12-31T23:59:59+00:00",
        };

        let mut stmt = conn.prepare(
            "SELECT d.id, d.account_id, d.message_id, d.kind, d.amount, d.currency,
                    d.due_at, d.past_due, d.source
             FROM deadlines d
             WHERE d.account_id = ?1
               AND d.due_at <= ?2
             ORDER BY d.due_at ASC",
        )?;
        let out = stmt
            .query_map(params![account_id, cutoff_ref], |r| {
                Ok(Deadline {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    message_id: r.get(2)?,
                    kind: r.get(3)?,
                    amount: r.get(4)?,
                    currency: r.get(5)?,
                    due_at: dt(r, 6)?,
                    past_due: r.get::<_, i64>(7)? != 0,
                    source: r.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    pub(super) fn ingest_message(&self, triaged: &TriagedMessage) -> Result<i64> {
        self.ingest_message_inner(triaged, None)
            .map(|id| id.expect("a plain ingest never refuses"))
    }

    /// See [`crate::store::Store::ingest_message_fresh`].
    pub(super) fn ingest_message_fresh(
        &self,
        triaged: &TriagedMessage,
        scope: HealScope,
    ) -> Result<Option<i64>> {
        self.ingest_message_inner(triaged, Some(scope))
    }

    /// The one ingest write. `heal` is the blank-body heal's flavour: refuse
    /// anything but a live, normal, non-spam row the store already holds, drop
    /// the stale vector, leave the unsubscribe ledger alone, and queue the
    /// agent job the scope asks for rather than the one the sync origin would.
    /// Everything else — the body replacement, the content-revision bump and
    /// the access reset that follows it — is what a re-ingest already does.
    fn ingest_message_inner(
        &self,
        triaged: &TriagedMessage,
        heal: Option<HealScope>,
    ) -> Result<Option<i64>> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;

        // 0. HEAL PRE-CHECK, on the LIVE row inside the transaction. The sweep
        //    snapshotted its candidates minutes or hours ago; the on-demand spam
        //    walk or a human seal may have ruled on the row since, and either
        //    outranks a re-read. `is_spam` is sticky-to-zero in the upsert
        //    below (a sighting outside SPAM clears it), so a fabricated
        //    `is_spam: false` from the sweep would otherwise flip a real
        //    verdict back — and put a spam body in front of a model.
        if heal.is_some() {
            let live: Option<(i64, i64, Option<String>)> = tx
                .query_row(
                    "SELECT m.id, m.is_spam, t.sensitivity
                     FROM messages m
                     LEFT JOIN triage t ON t.message_id = m.id
                     WHERE m.account_id = ?1 AND m.gmail_msg_id = ?2",
                    params![triaged.message.account_id, triaged.message.gmail_msg_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            match live {
                Some((_, 0, Some(sens))) if sens == "normal" => {}
                _ => return Ok(None),
            }
        }

        // 1. Upsert the message row (+ FTS).
        let id = upsert_message_conn(&tx, &triaged.message)?;

        // 1a. HEAL: the vector was computed from the text the row had, which
        //     was the subject alone; drop it so the vector backfill re-embeds
        //     from the real text, batched and throttled, off this path.
        if heal.is_some() {
            tx.execute(
                "DELETE FROM message_vecs WHERE message_id = ?1",
                params![id],
            )?;
        }

        // 1b. Contacts from Sent-mail To/Cc, in the SAME transaction.
        seed_contacts_conn(
            &tx,
            triaged.message.account_id,
            &triaged.recipients,
            &triaged.message.received_at.to_rfc3339(),
        )?;

        // 1b'. THE NORMALIZED RECIPIENT INDEX, from the FAITHFUL address set
        //      rather than the contact-filtered one beside it, and in the same
        //      transaction for the same reason: `message_recipients` is what
        //      send-group history joins against, so it must never describe a
        //      message the transaction rolled back. Received mail carries an
        //      empty set, which writes nothing and clears nothing (the row has
        //      none to clear).
        if triaged.message.is_sent {
            SqliteStore::sync_message_recipients_conn(
                &tx,
                triaged.message.account_id,
                id,
                &triaged.recipient_addrs,
            )?;
        }

        // 1c. UNSUBSCRIBE VIOLATION LEDGER: inbound mail from a sender the user
        //     unsubscribed from, past the 72h grace, bumps that sender's
        //     violation_count — in the SAME transaction as the message insert, so
        //     the ledger cannot drift from the mail that drives it.
        //     NOT on a heal: the message was counted when it arrived, and the
        //     bump is a blind `+ 1` with no idempotency key.
        if !triaged.message.is_sent && heal.is_none() {
            bump_unsub_violation_conn(
                &tx,
                triaged.message.account_id,
                &triaged.message.from_addr,
                triaged.message.received_at,
            )?;
        }

        // 2. Write the triage row IN THE SAME TRANSACTION. That is the whole
        //    point for sealed mail: sensitivity='sealed' commits atomically with
        //    the message, so there is no window in which it is queryable as
        //    normal mail. `model_used` stays NULL, which with
        //    sensitivity='normal' is the Stage-2 queue predicate.
        let deadline_dt = triaged.deadline.as_ref().map(|d| d.due_at.to_rfc3339());
        // Record kinds never decide attention state. Preserve explicit lifecycle.
        let now_s = Utc::now().to_rfc3339();
        let (status, resolved_at): (&str, Option<String>) = ("new", None);
        // Re-ingest PRESERVES the existing attention lifecycle: a re-sync must not
        // reopen an item the user dismissed. Receipt/calendar rows are the
        // exception, force-resolved on every ingest — the CASE keys off
        // `excluded.status`, and only auto-resolved rows pass 'done' in.
        //
        // Per-property Stage-1 reasons as JSON (NULL when empty, as for sealed /
        // sent mail). HUMAN-DOOR ONLY on read.
        let field_reasons_json = if triaged.field_reasons.is_empty() {
            None
        } else {
            serde_json::to_string(&triaged.field_reasons).ok()
        };
        // STAGE-1/STAGE-2 QUEUE MARKERS: `stage1_model_used` decides whether the
        // Stage-1 pass looks at this row, `needs_stage2` is the escalation seed.
        //   * Sealed / Sent / provider-spam: never queued for any LLM ('n/a').
        //     Sealed mail reaching a model is the one thing this system must
        //     never do; spam reaching one is the second, because spam bodies are
        //     attacker-written text and a Stage-1 prompt is a reader of them.
        //   * Filtered rule: skip Stage-1 and go straight to Stage-2, which is
        //     the only stage that evaluates `want_text` ('rule', needs_stage2=1).
        //   * EVERYTHING ELSE, rule-decided included: enter the Stage-1 queue
        //     (NULL), seeding `needs_stage2` from heuristic confidence.
        //
        // A Squelch/Surface rule USED TO stop here with 'rule' and never see a
        // model. It no longer does. The rule still wins — the user's own ruling
        // on a sender is not something a classifier gets to overturn — but it
        // decides ONE axis, and a row that skipped the model had no category, no
        // extraction, no deadline, and no revisit schedule either. Honoring an
        // instruction about visibility was quietly costing every other thing
        // triage knows how to produce.
        //
        // Filtered is told apart from Squelch/Surface by `confident`: the Filtered
        // rung parks NOT-confident precisely because its verdict is pending an
        // LLM read of `want_text` (see `triage::stage1`).
        let (stage1_model_used, needs_stage2): (Option<&str>, i64) = if triaged.sensitivity
            != Sensitivity::Normal
            || triaged.message.is_sent
            || triaged.message.is_spam
        {
            (Some("n/a"), 0)
        } else if triaged.matched_rule.is_some() && !triaged.confident {
            (Some("rule"), 1)
        } else {
            (None, if triaged.confident { 0 } else { 1 })
        };
        // RE-INGEST CLASSIFICATION GUARD. A re-ingest carries only HEURISTIC SEED
        // values, so for a row an LLM already classified (`model_used` set, or a
        // `stage1_model_used` other than the 'rule'/'n/a' sentinels) writing the
        // seed back would discard paid classification while the model markers
        // stay put — the row would never re-queue to recover it. This predicate
        // keeps those columns on conflict; still-seed rows refresh normally.
        const PROCESSED: &str = "(triage.model_used IS NOT NULL \
             OR (triage.stage1_model_used IS NOT NULL \
                 AND triage.stage1_model_used NOT IN ('rule', 'n/a')))";
        // DID A PERSON RULE ON *SENSITIVITY* FOR THIS ROW? Not "did a person
        // touch this row at all", which is what `triage.model_used = 'human'`
        // asks: `correct_triage` stamps that column for a TIER or a CATEGORY
        // correction too, so keying the seal freeze on it would pin the
        // sensitivity of every human-corrected row forever and take the
        // detector-fix direction below down with it — a better `detect_sealed`
        // could never newly seal a message whose tier somebody once fixed.
        //
        // `triage_feedback` is the exact record instead: `correct_triage`
        // writes one row per correction, naming the axis, in the SAME
        // transaction as the UPDATE, so a `dimension = 'sensitivity'` row is
        // "a person decided whether this is auth mail" and nothing else. It
        // rides `idx_triage_feedback_msg`, so this is an index probe on a table
        // that holds one row per human correction.
        //
        // BOTH DIRECTIONS, which is the point of asking about the axis rather
        // than the value: a person's seal outranks the seed that would revert
        // it (docs/SECURITY.md §4), and a person's un-seal outranks the same
        // recall-biased detector that over-sealed in the first place. An
        // un-seal a re-walk quietly undid would be a correction the user has to
        // make again every poll.
        const HUMAN_SENSITIVITY: &str = "EXISTS (SELECT 1 FROM triage_feedback f \
             WHERE f.account_id = triage.account_id \
               AND f.message_id = triage.message_id \
               AND f.dimension = 'sensitivity')";
        // SHIPMENTS-EXTRACTOR TRIGGER. 'pending' queues the row for the shipments
        // specialist; NULL means no shipping signal at ingest. Sealed, sent and
        // spam mail never queue — the detector does not even run for them.
        let ship_extract_model: Option<&str> = if triaged.ship_extract
            && triaged.sensitivity == Sensitivity::Normal
            && !triaged.message.is_sent
            && !triaged.message.is_spam
        {
            Some("pending")
        } else {
            None
        };
        let triage_upsert = format!(
            "INSERT INTO triage(message_id, account_id, importance, tier, sensitivity,
                 sealed_kind, one_line, reason, deadline, matched_rule_id,
                 stage1_model_used, needs_stage2, model_used,
                 status, resolved_at, created_at, field_reasons, ship_extract_model,
                 notify_eligible_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,NULL,?13,?14,?15,?16,?17,?18)
             ON CONFLICT(message_id) DO UPDATE SET
                 importance=CASE WHEN {PROCESSED} THEN triage.importance ELSE excluded.importance END,
                 tier=CASE WHEN {PROCESSED} THEN triage.tier ELSE excluded.tier END,
                 -- A HUMAN SEAL OUTRANKS A RE-INGEST, which every other verdict
                 -- column has always said via the PROCESSED predicate above and
                 -- this one did not.
                 -- A re-ingest carries only heuristic SEED values, and the seed
                 -- detector never saw whatever made a person call this mail
                 -- auth, so `excluded.sensitivity` here reverts the seal — on a
                 -- catch-up, or on the routine INBOX re-walk a held cursor
                 -- causes. That is not a bookkeeping loss: an unsealed row is a
                 -- row the fast lane will send to a model and turn into an
                 -- events row, minted AFTER `correct_triage` ran its redaction
                 -- and so with nothing left that can redact it (docs/SECURITY.md
                 -- §4). HUMAN_SENSITIVITY rather than PROCESSED because only a
                 -- PERSON outranks a person: an LLM-classified row refreshes
                 -- from the fresh detection as before, which is what lets a
                 -- detector fix newly seal an old message.
                 sensitivity=CASE WHEN {HUMAN_SENSITIVITY}
                     THEN triage.sensitivity ELSE excluded.sensitivity END,
                 -- SEALED_KIND IS NOT FROZEN WITH IT, deliberately. It looks
                 -- like it should move with the column above, and freezing it
                 -- only loses: `correct_triage` never writes a kind, so a row
                 -- a PERSON sealed carries NULL until a later detection
                 -- supplies one, and that kind is what routes the tap to the
                 -- reveal flow. Nothing reads it ungated either — every query
                 -- that selects it (`sealed_messages`, `sealed_body`, the
                 -- shred sweep) joins on `sensitivity = 'sealed'` — so a stale
                 -- kind left on a row a person called normal is inert.
                 sealed_kind=excluded.sealed_kind,
                 one_line=CASE WHEN {PROCESSED} THEN triage.one_line ELSE excluded.one_line END,
                 reason=CASE WHEN {PROCESSED} THEN triage.reason ELSE excluded.reason END,
                 field_reasons=CASE WHEN {PROCESSED} THEN triage.field_reasons ELSE excluded.field_reasons END,
                 deadline=CASE WHEN {PROCESSED} THEN triage.deadline ELSE excluded.deadline END,
                 matched_rule_id=excluded.matched_rule_id,
                 -- A PROCESSED shipments marker survives re-ingest (re-running a
                 -- paid extractor on the same mail buys nothing); a NULL or a
                 -- still-'pending' one refreshes from the fresh detection, so a
                 -- detector fix can newly queue — or newly un-queue — a row.
                 ship_extract_model = CASE
                     WHEN triage.ship_extract_model IS NOT NULL
                          AND triage.ship_extract_model != 'pending'
                     THEN triage.ship_extract_model ELSE excluded.ship_extract_model END,
                 status=CASE WHEN excluded.status='done' THEN 'done' ELSE triage.status END,
                 resolved_at=CASE WHEN excluded.status='done'
                     THEN excluded.resolved_at ELSE triage.resolved_at END,
                 -- WRITTEN ONCE, ON FIRST INSERT, AND NEVER AGAIN — not even to
                 -- a fresher stamp, and not conditioned on `PROCESSED` like the
                 -- verdict columns. Notify eligibility is a fact about when we
                 -- FIRST saw a message, so a re-ingest (history overlap, a
                 -- catch-up re-scan, a sealed row being rewritten) must not be
                 -- able to move it forward: that would hand the whole re-scanned
                 -- window a fresh hour of notify eligibility, which is the storm
                 -- the guard exists to prevent. A NULL stays NULL for the same
                 -- reason, from the other direction.
                 notify_eligible_at = triage.notify_eligible_at"
        );
        tx.execute(
            &triage_upsert,
            params![
                id,
                triaged.message.account_id,
                triaged.importance as i64,
                triaged.tier.as_str(),
                triaged.sensitivity.as_str(),
                triaged.sealed_kind.map(|k| k.as_str()),
                triaged.one_line,
                triaged.reason,
                deadline_dt,
                triaged.matched_rule,
                stage1_model_used,
                needs_stage2,
                status,
                resolved_at,
                now_s,
                field_reasons_json,
                ship_extract_model,
                triaged.notify_eligible_at.map(|t| t.to_rfc3339()),
            ],
        )?;

        // 3. Deadlines: non-sealed mail only (Stage-1 never runs on sealed
        //    content), so a sealed re-ingest passes None and only clears.
        let ingest_deadline = if triaged.sensitivity == Sensitivity::Sealed {
            None
        } else {
            triaged.deadline.as_ref()
        };
        rewrite_deadline_conn(&tx, triaged.message.account_id, id, ingest_deadline)?;

        // 4. Shipment: NON-SEALED mail only, so `shipments` is sealed-free by
        //    construction. Upserted in the SAME transaction so a package's state
        //    and its source message land atomically.
        if triaged.sensitivity != Sensitivity::Sealed
            && let Some(s) = &triaged.shipment
        {
            upsert_shipment_conn(
                &tx,
                triaged.message.account_id,
                id,
                s,
                triaged.message.received_at,
            )?;
        }

        // 5. Receipt: NON-SEALED mail only, so `receipts` is sealed-free by
        //    construction. Independent of shipment detection — an order
        //    confirmation with a total AND tracking lands in BOTH tables.
        if triaged.sensitivity != Sensitivity::Sealed
            && let Some(r) = &triaged.receipt
        {
            upsert_receipt_conn(
                &tx,
                triaged.message.account_id,
                id,
                &triaged.message.from_addr,
                triaged.message.from_name.as_deref(),
                r,
                triaged.message.received_at,
            )?;
        }

        // 6. Calendar update: NON-SEALED mail only, so `calendar_updates` is
        //    sealed-free by construction. Independent of the other detectors,
        //    exactly like receipts. Nothing is written back to Gmail — "resolved"
        //    is squelch-internal.
        if triaged.sensitivity != Sensitivity::Sealed
            && let Some(c) = &triaged.calendar
        {
            upsert_calendar_conn(
                &tx,
                triaged.message.account_id,
                id,
                c,
                triaged.message.received_at,
            )?;
        }

        // 7. Attachments: written for sealed mail too — the byte-serving endpoint
        //    guards sealed parents. Replaces prior rows so re-ingest is
        //    idempotent; over-cap parts store a NULL blob.
        insert_attachments_conn(&tx, triaged.message.account_id, id, &triaged.attachments)?;
        // 8. THE AGENT'S BOOKKEEPING, last, over the row as it now stands: it
        //    re-reads the message, and a changed content snapshot is what
        //    advances the revision and queues the work. The trigger names the
        //    reason. A heal is neither an arrival nor a backfill: `heal` queues
        //    a background investigation of the re-read row, and the text-only
        //    scope queues an access reassessment alone (`source_access` is the
        //    trigger the store already resolves to an `access` job).
        let trigger = match heal {
            None if triaged.foreground_triage => "ingest",
            None => "backfill",
            Some(HealScope::TextAndTriage) => "heal",
            Some(HealScope::Text) => "source_access",
        };
        super::agent_triage::enqueue_agent_triage_conn(
            &tx,
            triaged.message.account_id,
            id,
            trigger,
            triaged.notify_eligible_at.is_some(),
        )?;

        tx.commit()?;
        Ok(Some(id))
    }

    pub(super) fn is_known_contact(&self, account_id: AccountId, addr: &str) -> Result<bool> {
        let conn = self.lock()?;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM contacts
             WHERE account_id=?1 AND addr=?2 COLLATE NOCASE AND sent_count > 0",
            params![account_id, addr],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub(super) fn sync_state(
        &self,
        account_id: AccountId,
        mailbox: &str,
    ) -> Result<Option<SyncState>> {
        let conn = self.lock()?;
        let row = conn
            .query_row(
                "SELECT uidvalidity, last_uid FROM sync_state
                 WHERE account_id=?1 AND mailbox=?2",
                params![account_id, mailbox],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(row.map(|(uv, lu)| SyncState {
            uidvalidity: uv as u32,
            last_uid: lu as u64,
        }))
    }

    pub(super) fn set_sync_state(
        &self,
        account_id: AccountId,
        mailbox: &str,
        state: &SyncState,
    ) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO sync_state(account_id, mailbox, uidvalidity, last_uid)
             VALUES(?1,?2,?3,?4)
             ON CONFLICT(account_id, mailbox) DO UPDATE SET
                 uidvalidity=excluded.uidvalidity, last_uid=excluded.last_uid",
            params![
                account_id,
                mailbox,
                state.uidvalidity as i64,
                state.last_uid as i64,
            ],
        )?;
        Ok(())
    }

    pub(super) fn inbox_unread(&self, account_id: AccountId) -> Result<Option<InboxUnread>> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT messages, threads, fetched_at FROM inbox_unread WHERE account_id=?1",
            params![account_id],
            |r| {
                Ok(InboxUnread {
                    messages: r.get(0)?,
                    threads: r.get(1)?,
                    fetched_at: dt(r, 2)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    pub(super) fn set_inbox_unread(
        &self,
        account_id: AccountId,
        messages: i64,
        threads: i64,
    ) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO inbox_unread(account_id, messages, threads, fetched_at)
             VALUES(?1,?2,?3,?4)
             ON CONFLICT(account_id) DO UPDATE SET
                 messages=excluded.messages,
                 threads=excluded.threads,
                 fetched_at=excluded.fetched_at",
            params![account_id, messages, threads, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub(super) fn sealed_messages(&self, account_id: AccountId) -> Result<Vec<SealedMessage>> {
        // Human Auth lookup: canonical actionable auth plus unmigrated legacy rows.
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT m.id, m.account_id, m.thread_id, m.from_addr, m.subject,
                    m.received_at, CASE WHEN s.message_id IS NOT NULL THEN CASE json_extract(d.decision_json,'$.auth.kinds[0]') WHEN 'sign_in_link' THEN 'magic_link' ELSE json_extract(d.decision_json,'$.auth.kinds[0]') END ELSE t.sealed_kind END
             FROM messages m
             LEFT JOIN triage t ON t.message_id = m.id AND t.account_id=m.account_id
             LEFT JOIN agent_message_state s ON s.message_id=m.id AND s.account_id=m.account_id
             LEFT JOIN agent_message_decisions d ON d.message_id=m.id AND d.account_id=m.account_id
             WHERE m.account_id = ?1 AND ((s.access='restricted' AND json_array_length(d.decision_json,'$.auth.kinds')>0) OR (s.message_id IS NULL AND t.sensitivity='sealed'))
             ORDER BY m.received_at DESC",
        )?;
        let out = stmt
            .query_map(params![account_id], |r| {
                Ok(SealedMessage {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    thread_id: r.get(2)?,
                    from_addr: r.get(3)?,
                    subject: r.get(4)?,
                    received_at: dt(r, 5)?,
                    sealed_kind: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    pub(super) fn sealed_body(&self, account_id: AccountId, message_id: i64) -> Result<SealedBody> {
        // Human Auth lookup detail. Ordinary email reads remain unrestricted.
        let conn = self.lock()?;
        let row = conn
            .query_row(
                "SELECT m.id, m.account_id, m.thread_id, m.from_addr, m.from_name,
                        m.subject, m.received_at, CASE WHEN s.message_id IS NOT NULL THEN CASE json_extract(d.decision_json,'$.auth.kinds[0]') WHEN 'sign_in_link' THEN 'magic_link' ELSE json_extract(d.decision_json,'$.auth.kinds[0]') END ELSE t.sealed_kind END, m.body, m.body_html
                 FROM messages m
                 LEFT JOIN triage t ON t.message_id = m.id AND t.account_id=m.account_id
             LEFT JOIN agent_message_state s ON s.message_id=m.id AND s.account_id=m.account_id
             LEFT JOIN agent_message_decisions d ON d.message_id=m.id AND d.account_id=m.account_id
                 WHERE m.account_id = ?1 AND m.id = ?2 AND ((s.access='restricted' AND json_array_length(d.decision_json,'$.auth.kinds')>0) OR (s.message_id IS NULL AND t.sensitivity='sealed'))",
                params![account_id, message_id],
                |r| {
                    Ok(SealedBody {
                        id: r.get(0)?,
                        account_id: r.get(1)?,
                        thread_id: r.get(2)?,
                        from_addr: r.get(3)?,
                        from_name: r.get(4)?,
                        subject: r.get(5)?,
                        received_at: dt(r, 6)?,
                        sealed_kind: r.get(7)?,
                        body: r.get(8)?,
                        body_html: r.get(9)?,
                    })
                },
            )
            .optional()?;
        row.ok_or(CoreError::NotFound)
    }

    pub(super) fn sent_listing(
        &self,
        account_id: AccountId,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<SentMessage>> {
        let conn = self.lock()?;
        // Human-only outbox: pending and restricted mail remain readable.
        // Recipients and read receipts are scoped to the owning account.
        let mut stmt = conn.prepare(
            "SELECT m.id, m.thread_id, COALESCE(m.to_addrs, ''), m.subject, m.snippet,
                    m.received_at,
                    (SELECT COUNT(*) FROM message_opens o
                     JOIN send_trackers st ON st.token = o.token
                     WHERE st.account_id = m.account_id AND st.message_id = m.id) AS opens
             FROM messages m
             WHERE m.account_id = ?1
               AND m.is_sent = 1
             ORDER BY m.received_at DESC, m.id DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let out = stmt
            .query_map(params![account_id, limit as i64, offset as i64], |r| {
                Ok(SentMessage {
                    id: r.get(0)?,
                    thread_id: r.get(1)?,
                    to: r.get(2)?,
                    subject: r.get(3)?,
                    snippet: r.get(4)?,
                    // The stored string verbatim — this is the same RFC3339 the
                    // rest of the door serves, and re-formatting it would only
                    // invent a second spelling of one timestamp.
                    sent_at: r.get(5)?,
                    opens: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    pub(super) fn sent_missing_recipients(
        &self,
        account_id: AccountId,
        limit: u32,
    ) -> Result<Vec<SentMissingRecipients>> {
        let conn = self.lock()?;
        // The recipients-backfill queue: sent rows ingested before `to_addrs`
        // existed. Newest first, so an interrupted pass has already covered the
        // mail the user is most likely to look at.
        let mut stmt = conn.prepare(
            "SELECT id, gmail_msg_id FROM messages
             WHERE account_id = ?1 AND is_sent = 1 AND to_addrs IS NULL
             ORDER BY received_at DESC, id DESC
             LIMIT ?2",
        )?;
        let out = stmt
            .query_map(params![account_id, limit as i64], |r| {
                Ok(SentMissingRecipients {
                    message_id: r.get(0)?,
                    gmail_msg_id: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// See [`crate::store::Store::blank_body_messages`].
    pub(super) fn blank_body_messages(
        &self,
        account_id: AccountId,
        before_id: i64,
        limit: u32,
    ) -> Result<crate::store::BlankBodyScan> {
        let conn = self.lock()?;
        // `body_html IS NOT NULL` is the only shape this heal can recover — a
        // plain-text-only message with nothing in it has nothing to flatten —
        // and it is a column test, so it bounds the read to HTML mail without
        // re-expressing the blankness predicate. Bodies are streamed one row
        // at a time and judged in Rust; only the blank ones are kept.
        let mut stmt = conn.prepare(
            "SELECT m.id, m.gmail_msg_id, m.received_at, m.body
             FROM messages m
             JOIN triage t ON t.message_id = m.id
             WHERE m.account_id = ?1
               AND m.id < ?2
               AND m.body_html IS NOT NULL
               AND m.is_sent = 0
               AND m.is_spam = 0
               AND t.sensitivity = 'normal'
             ORDER BY m.id DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![account_id, before_id, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                dt(r, 2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut scan = crate::store::BlankBodyScan::default();
        let mut seen = 0u32;
        let mut last_id = None;
        for row in rows {
            let (message_id, gmail_msg_id, received_at, body) = row?;
            seen += 1;
            last_id = Some(message_id);
            // A NULL body is nothing to read, the same as a blank one.
            if crate::triage::text::is_blank(body.as_deref().unwrap_or("")) {
                scan.candidates.push(crate::store::BlankBodyMessage {
                    message_id,
                    gmail_msg_id,
                    received_at,
                });
            }
        }
        // A short chunk means the scan reached the oldest row; a full one may
        // have more below it, and the next chunk starts under the last id seen.
        scan.next_before_id = if seen < limit { None } else { last_id };
        Ok(scan)
    }

    pub(super) fn set_message_to_addrs(
        &self,
        account_id: AccountId,
        message_id: i64,
        to_addrs: &str,
    ) -> Result<bool> {
        let conn = self.lock()?;
        // `is_sent = 1` in the predicate, not just in the caller: recipients are
        // a property of mail the user SENT, and a backfill that ever pointed at
        // an inbound row must write nothing rather than invent a "to" for it.
        // Writing "" is meaningful — it takes the row out of the backfill queue
        // as "looked, and the headers named nobody".
        let n = conn.execute(
            "UPDATE messages SET to_addrs = ?3
             WHERE account_id = ?1 AND id = ?2 AND is_sent = 1",
            params![account_id, message_id, to_addrs],
        )?;
        // Keep the normalized index in step with the column it is derived from.
        // Only when the UPDATE matched: a row this backfill declined to touch
        // (not sent, not ours) must not gain recipients either.
        //
        // Parsed back OUT of the display string because that is all this path
        // ever has — the sweep hands over a rendered header, not the mailboxes.
        // The ingest path, which does have them, never comes through here.
        if n > 0 {
            let addrs = crate::sync::ingest::parse_stored_recipients(to_addrs);
            Self::sync_message_recipients_conn(&conn, account_id, message_id, &addrs)?;
        }
        Ok(n > 0)
    }
}
