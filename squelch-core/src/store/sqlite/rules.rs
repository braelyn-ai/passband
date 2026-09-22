//! Sender rules and the unsubscribe ledger.

use super::*;

/// Validate a sender-rule write, on the path of every door. A FILTERED rule with
/// an empty `want_text` is a contradiction (there is no standing instruction to
/// filter by): Stage-2 would get no instruction to evaluate, so the rule would
/// silently degrade while still reading as a rule in the UI. The message is
/// client-visible, so it stays polarity-blind: `want_text` may name what the
/// owner wants OR what they do not care about.
fn validate_sender_rule(want_text: &str, disposition: Disposition) -> Result<()> {
    if disposition == Disposition::Filtered && want_text.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "a filtered rule needs a want_text saying what you do, or do not, want from this sender"
                .into(),
        ));
    }
    Ok(())
}

/// Map a UNIQUE-constraint failure into a client-legible [`CoreError::InvalidInput`].
/// `sender_rules` carries `UNIQUE(account_id, match_pattern)`, so editing one
/// rule onto another's pattern is a user mistake, not a server fault: without
/// this it surfaces as a raw sqlite error that squelch-api collapses to a 500.
fn map_pattern_conflict(e: rusqlite::Error) -> CoreError {
    match &e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            CoreError::InvalidInput("a rule for that pattern already exists".into())
        }
        _ => CoreError::from(e),
    }
}

/// A preference change requests fresh reasoning; it never rewrites placement.
/// Queue messages already in the new pipeline. Historical cutover reads the
/// current preferences when it eventually reaches untouched messages.
fn queue_preference_change(conn: &Connection, account: AccountId, patterns: &[&str]) -> Result<()> {
    let ids = {
        let mut statement = conn.prepare(
            "SELECT m.id, m.from_addr FROM messages m
             JOIN agent_message_state a ON a.account_id=m.account_id AND a.message_id=m.id
             WHERE m.account_id=?1",
        )?;
        statement
            .query_map([account], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let change_id: String = conn.query_row("SELECT lower(hex(randomblob(8)))", [], |r| r.get(0))?;
    for (id, sender) in ids {
        if patterns
            .iter()
            .any(|pattern| crate::triage::rules::glob_match(pattern, &sender))
        {
            super::agent_triage::enqueue_agent_triage_conn(
                conn,
                account,
                id,
                &format!("preference:{change_id}"),
                false,
            )?;
        }
    }
    Ok(())
}

impl SqliteStore {
    pub(super) fn set_sender_rule(
        &self,
        account_id: AccountId,
        match_pattern: &str,
        want_text: &str,
        disposition: Disposition,
    ) -> Result<i64> {
        validate_sender_rule(want_text, disposition)?;
        let mut connection = self.lock()?;
        let conn = connection.transaction()?;
        conn.execute(
            "INSERT INTO sender_rules(account_id, match_pattern, want_text, disposition, updated_at)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(account_id, match_pattern) DO UPDATE SET
                 want_text=excluded.want_text, disposition=excluded.disposition,
                 updated_at=excluded.updated_at",
            params![
                account_id,
                match_pattern,
                want_text,
                disposition.as_str(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM sender_rules WHERE account_id=?1 AND match_pattern=?2",
            params![account_id, match_pattern],
            |r| r.get(0),
        )?;
        queue_preference_change(&conn, account_id, &[match_pattern])?;
        conn.commit()?;
        Ok(id)
    }

    pub(super) fn set_sender_rule_audited(
        &self,
        account_id: AccountId,
        match_pattern: &str,
        want_text: &str,
        disposition: Disposition,
        audit: &NewAuditEntry,
    ) -> Result<i64> {
        validate_sender_rule(want_text, disposition)?;
        // FAIL-CLOSED: the rule write and its audit row share ONE transaction, so
        // a failed audit INSERT rolls the rule back and an agent-door write can
        // never land untraced.
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO sender_rules(account_id, match_pattern, want_text, disposition, updated_at)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(account_id, match_pattern) DO UPDATE SET
                 want_text=excluded.want_text, disposition=excluded.disposition,
                 updated_at=excluded.updated_at",
            params![
                account_id,
                match_pattern,
                want_text,
                disposition.as_str(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        let id: i64 = tx.query_row(
            "SELECT id FROM sender_rules WHERE account_id=?1 AND match_pattern=?2",
            params![account_id, match_pattern],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO audit_log(account_id, ts, actor, action, target, detail)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                account_id,
                Utc::now().to_rfc3339(),
                audit.actor,
                audit.action,
                audit.target,
                audit.detail,
            ],
        )?;
        queue_preference_change(&tx, account_id, &[match_pattern])?;
        tx.commit()?;
        Ok(id)
    }

    pub(super) fn update_sender_rule(
        &self,
        account_id: AccountId,
        id: i64,
        match_pattern: &str,
        want_text: &str,
        disposition: Disposition,
    ) -> Result<bool> {
        validate_sender_rule(want_text, disposition)?;
        let mut connection = self.lock()?;
        let conn = connection.transaction()?;
        let old_pattern: Option<String> = conn
            .query_row(
                "SELECT match_pattern FROM sender_rules WHERE account_id=?1 AND id=?2",
                params![account_id, id],
                |r| r.get(0),
            )
            .optional()?;
        // Retargeting this rule onto a pattern another rule already owns trips
        // UNIQUE(account_id, match_pattern); map it to InvalidInput so the door
        // returns a 4xx the user can act on instead of a 500.
        let n = conn
            .execute(
                "UPDATE sender_rules SET
                 match_pattern = ?3, want_text = ?4, disposition = ?5, updated_at = ?6
             WHERE account_id = ?1 AND id = ?2",
                params![
                    account_id,
                    id,
                    match_pattern,
                    want_text,
                    disposition.as_str(),
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(map_pattern_conflict)?;
        if let Some(old_pattern) = old_pattern {
            queue_preference_change(&conn, account_id, &[&old_pattern, match_pattern])?;
        }
        conn.commit()?;
        Ok(n > 0)
    }

    pub(super) fn list_sender_rules(&self, account_id: AccountId) -> Result<Vec<SenderRule>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, account_id, match_pattern, want_text, disposition, updated_at
             FROM sender_rules WHERE account_id=?1 ORDER BY updated_at DESC",
        )?;
        let out = stmt
            .query_map(params![account_id], |r| {
                Ok(SenderRule {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    match_pattern: r.get(2)?,
                    want_text: r.get(3)?,
                    disposition: Disposition::parse(&r.get::<_, String>(4)?)
                        .unwrap_or(Disposition::Surface),
                    updated_at: dt(r, 5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    pub(super) fn delete_sender_rule(&self, account_id: AccountId, id: i64) -> Result<bool> {
        let mut connection = self.lock()?;
        let conn = connection.transaction()?;
        let pattern: Option<String> = conn
            .query_row(
                "SELECT match_pattern FROM sender_rules WHERE account_id=?1 AND id=?2",
                params![account_id, id],
                |r| r.get(0),
            )
            .optional()?;
        let n = conn.execute(
            "DELETE FROM sender_rules WHERE account_id=?1 AND id=?2",
            params![account_id, id],
        )?;
        if let Some(pattern) = pattern {
            queue_preference_change(&conn, account_id, &[&pattern])?;
        }
        conn.commit()?;
        Ok(n > 0)
    }

    pub(super) fn message_unsub_fields(
        &self,
        account_id: AccountId,
        message_id: i64,
    ) -> Result<Option<MessageUnsub>> {
        let conn = self.lock()?;
        // This is a human action. Agent restrictions do not hide unsubscribe metadata.
        let row = conn
            .query_row(
                "SELECT m.from_addr, m.list_unsubscribe, m.list_unsub_one_click, m.body_html
                 FROM messages m
                 WHERE m.account_id = ?1 AND m.id = ?2",
                params![account_id, message_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(
            |(from_addr, list_unsubscribe, one_click, body_html)| MessageUnsub {
                from_addr,
                list_unsubscribe,
                list_unsub_one_click: one_click != 0,
                body_html,
            },
        ))
    }

    pub(super) fn upsert_unsubscribe(
        &self,
        account_id: AccountId,
        sender: &str,
        method: &str,
        source_message_id: Option<i64>,
        requested_at: DateTime<Utc>,
    ) -> Result<()> {
        let conn = self.lock()?;
        // A fresh request RESETS the ledger — the user re-asked, so the 72h grace
        // clock restarts from this `requested_at`.
        conn.execute(
            "INSERT INTO unsubscribes(account_id, sender_addr, requested_at, method,
                 source_message_id, violation_count, last_violation_at, resolution)
             VALUES(?1,?2,?3,?4,?5,0,NULL,NULL)
             ON CONFLICT(account_id, sender_addr) DO UPDATE SET
                 requested_at=excluded.requested_at, method=excluded.method,
                 source_message_id=excluded.source_message_id,
                 violation_count=0, last_violation_at=NULL, resolution=NULL",
            params![
                account_id,
                sender,
                requested_at.to_rfc3339(),
                method,
                source_message_id,
            ],
        )?;
        Ok(())
    }

    pub(super) fn list_unsubscribes(
        &self,
        account_id: AccountId,
    ) -> Result<Vec<UnsubscribeRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT sender_addr, requested_at, method, violation_count,
                    last_violation_at, resolution
             FROM unsubscribes
             WHERE account_id = ?1
             ORDER BY requested_at DESC",
        )?;
        let out = stmt
            .query_map(params![account_id], |r| {
                Ok(UnsubscribeRecord {
                    sender: r.get(0)?,
                    requested_at: dt(r, 1)?,
                    method: r.get(2)?,
                    violation_count: r.get(3)?,
                    last_violation_at: dt_opt(r, 4)?,
                    resolution: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    pub(super) fn set_unsubscribe_resolution(
        &self,
        account_id: AccountId,
        sender: &str,
        resolution: &str,
    ) -> Result<bool> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE unsubscribes SET resolution = ?3
             WHERE account_id = ?1 AND sender_addr = ?2",
            params![account_id, sender, resolution],
        )?;
        Ok(n > 0)
    }
}
