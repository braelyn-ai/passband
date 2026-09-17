//! Ingest, thread view, attachment, sealed-read and Gmail-counter tests.

use super::super::*;
use super::support::*;
use crate::types::{SealedKind, Sensitivity, Tier};

/// Install an explicit model access assessment for a storage fixture. Legacy
/// sensitivity is deliberately independent, so tests cannot accidentally use it
/// as the new external permission boundary.
pub(super) fn assess_external(store: &SqliteStore, account: AccountId, message: i64, access: &str) {
    use crate::store::agent_triage::AgentTriageStore;
    store
        .enqueue_agent_triage(account, message, "test-access", false)
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE agent_message_state SET access=?1 WHERE account_id=?2 AND message_id=?3",
            params![access, account, message],
        )
        .unwrap();
}

#[test]
fn thread_subject_seeks_thread_in_date_order_after_reopening_old_store() {
    let dir = std::env::temp_dir().join(format!("squelch-thread-index-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mail.db");
    let acct;
    {
        let store = SqliteStore::open(&path).unwrap();
        acct = store.ensure_account("me@example.com").unwrap();
        let now = Utc::now();
        triaged(acct, "later", "thread")
            .subject("Later subject")
            .received_at(now)
            .seed(&store);
        triaged(acct, "earlier", "thread")
            .subject("Original subject")
            .received_at(now - chrono::Duration::days(1))
            .seed(&store);
        // Simulate the previous schema. Opening an existing mailbox must add
        // the index too, without requiring a new database or a data backfill.
        store
            .lock()
            .unwrap()
            .execute_batch("DROP INDEX idx_messages_thread_received")
            .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    let view = store.thread_view_with_html(acct, "thread").unwrap();
    assert_eq!(view.subject, "Original subject");
    assert_eq!(view.messages.len(), 2);
    assert_eq!(view.messages[0].subject, "Original subject");
    assert_eq!(view.messages[1].subject, "Later subject");
    let conn = store.lock().unwrap();
    let sql = format!(
        "EXPLAIN QUERY PLAN {}",
        super::super::messages::THREAD_SUBJECT_SQL
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    let plan: Vec<String> = stmt
        .query_map(params![acct, "thread"], |r| r.get(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        plan.iter().any(|step| step.contains("SEARCH messages")
            && step.contains("account_id=? AND thread_id=?")),
        "thread lookup must not scan the account: {plan:?}"
    );
    assert!(
        !plan.iter().any(|step| step.contains("TEMP B-TREE")),
        "thread index must supply the date order: {plan:?}"
    );
    drop(stmt);
    drop(conn);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ingest_message_persists_attachments_and_thread_view_carries_them() {
    use crate::config::Stage1Config;
    let (store, acct) = store();

    let eml = "From: S <s@ex.com>\r\n\
               To: me@example.com\r\n\
               Subject: files\r\n\
               Date: Mon, 7 Jul 2026 10:00:00 +0000\r\n\
               MIME-Version: 1.0\r\n\
               Content-Type: multipart/mixed; boundary=\"B\"\r\n\
               \r\n\
               --B\r\nContent-Type: text/plain\r\n\r\nbody\r\n\
               --B\r\nContent-Type: application/pdf\r\n\
               Content-Disposition: attachment; filename=\"doc.pdf\"\r\n\
               Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n\
               --B--\r\n";
    let fetched = crate::sync::ingest::RawFetched {
        account_id: acct,
        gmail_msg_id: "g1".into(),
        gmail_thread_id: Some("t1".into()),
        raw: eml.as_bytes().to_vec(),
        internal_date: Some(Utc::now()),
        is_sent: false,
        is_spam: false,
        account_addr: "me@example.com".into(),
    };
    let t = crate::sync::ingest::ingest(&fetched, &Stage1Config::default(), Utc::now(), |_| false);
    assert_eq!(t.attachments.len(), 1, "one pdf attachment extracted");
    let mid = store.ingest_message(&t).unwrap();

    // Thread view carries the attachment metadata (downloadable = true).
    let view = store.thread_view_with_html(acct, "t1").unwrap();
    assert_eq!(view.messages.len(), 1);
    let atts = &view.messages[0].attachments;
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0].filename, "doc.pdf");
    assert_eq!(atts[0].mime, "application/pdf");
    assert_eq!(atts[0].size, 5);
    assert!(atts[0].downloadable);

    // Bytes come back through attachment_bytes.
    let got = store
        .attachment_bytes(acct, atts[0].id)
        .unwrap()
        .expect("bytes");
    assert_eq!(got.0, "doc.pdf");
    assert_eq!(got.2.as_deref(), Some(&b"Hello"[..]));

    // Re-ingest is idempotent: still exactly one attachment row.
    let mid2 = store.ingest_message(&t).unwrap();
    assert_eq!(mid, mid2);
    let view2 = store.thread_view_with_html(acct, "t1").unwrap();
    assert_eq!(
        view2.messages[0].attachments.len(),
        1,
        "re-ingest must not duplicate"
    );
}

#[test]
fn cid_inline_part_carries_its_content_id_to_the_thread_view() {
    // The whole point of the column: the body keeps <img src="cid:logo@squelch">,
    // so the attachment row that fills it has to be findable BY that token.
    use crate::config::Stage1Config;
    let (store, acct) = store();

    let eml = "From: S <s@ex.com>\r\n\
               To: me@example.com\r\n\
               Subject: newsletter\r\n\
               Date: Mon, 7 Jul 2026 10:00:00 +0000\r\n\
               MIME-Version: 1.0\r\n\
               Content-Type: multipart/related; boundary=\"B\"\r\n\
               \r\n\
               --B\r\nContent-Type: text/html\r\n\r\n\
               <p><img src=\"cid:logo@squelch\"></p>\r\n\
               --B\r\nContent-Type: image/png\r\n\
               Content-ID: <logo@squelch>\r\n\
               Content-Disposition: inline\r\n\
               Content-Transfer-Encoding: base64\r\n\r\naW5saW5l\r\n\
               --B\r\nContent-Type: application/pdf\r\n\
               Content-Disposition: attachment; filename=\"doc.pdf\"\r\n\
               Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n\
               --B--\r\n";
    let fetched = crate::sync::ingest::RawFetched {
        account_id: acct,
        gmail_msg_id: "g-cid".into(),
        gmail_thread_id: Some("t-cid".into()),
        raw: eml.as_bytes().to_vec(),
        internal_date: Some(Utc::now()),
        is_sent: false,
        is_spam: false,
        account_addr: "me@example.com".into(),
    };
    let t = crate::sync::ingest::ingest(&fetched, &Stage1Config::default(), Utc::now(), |_| false);
    store.ingest_message(&t).unwrap();

    let view = store.thread_view_with_html(acct, "t-cid").unwrap();
    let atts = &view.messages[0].attachments;
    assert_eq!(atts.len(), 2, "inline image + pdf");
    let inline = atts
        .iter()
        .find(|a| a.mime == "image/png")
        .expect("inline image row");
    assert_eq!(inline.content_id.as_deref(), Some("logo@squelch"));
    let pdf = atts
        .iter()
        .find(|a| a.filename == "doc.pdf")
        .expect("pdf row");
    assert!(
        pdf.content_id.is_none(),
        "a real attachment carries no cid to resolve"
    );
}

#[test]
fn double_attached_identical_file_cannot_kill_ingest() {
    // REMOTE INGEST DoS: two parts with the SAME filename and size violate
    // the UNIQUE key, which must collapse to one row rather than roll back
    // the whole message ingest.
    use crate::config::Stage1Config;
    let (store, acct) = store();

    let eml = "From: S <s@ex.com>\r\n\
               To: me@example.com\r\n\
               Subject: dup\r\n\
               Date: Mon, 7 Jul 2026 10:00:00 +0000\r\n\
               MIME-Version: 1.0\r\n\
               Content-Type: multipart/mixed; boundary=\"B\"\r\n\
               \r\n\
               --B\r\nContent-Type: text/plain\r\n\r\nbody\r\n\
               --B\r\nContent-Type: application/pdf\r\n\
               Content-Disposition: attachment; filename=\"doc.pdf\"\r\n\
               Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n\
               --B\r\nContent-Type: application/pdf\r\n\
               Content-Disposition: attachment; filename=\"doc.pdf\"\r\n\
               Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n\
               --B--\r\n";
    let fetched = crate::sync::ingest::RawFetched {
        account_id: acct,
        gmail_msg_id: "g-dup".into(),
        gmail_thread_id: Some("t-dup".into()),
        raw: eml.as_bytes().to_vec(),
        internal_date: Some(Utc::now()),
        is_sent: false,
        is_spam: false,
        account_addr: "me@example.com".into(),
    };
    let t = crate::sync::ingest::ingest(&fetched, &Stage1Config::default(), Utc::now(), |_| false);
    assert_eq!(t.attachments.len(), 2, "both parts extracted");
    // The ingest MUST NOT error — identical duplicates collapse to one row.
    store
        .ingest_message(&t)
        .expect("duplicate attachments must not fail ingest");
    let view = store.thread_view_with_html(acct, "t-dup").unwrap();
    assert_eq!(view.messages[0].attachments.len(), 1);
}

#[test]
fn auth_pass_round_trips_through_the_thread_view_and_self_heals_on_re_upsert() {
    let (store, acct) = store();
    // Pinned so the view's received_at ordering is stable across the re-upsert.
    let at = |n: i64| Utc::now() - chrono::Duration::minutes(10 - n);

    // Pre-existing mail: never evaluated, so it rests at NULL.
    triaged(acct, "g-null", "t-auth")
        .received_at(at(1))
        .upsert(&store);
    // Evaluated mail, both verdicts.
    triaged(acct, "g-pass", "t-auth")
        .received_at(at(2))
        .auth_pass(Some(true))
        .upsert(&store);
    triaged(acct, "g-fail", "t-auth")
        .received_at(at(3))
        .auth_pass(Some(false))
        .upsert(&store);

    let view = store.thread_view_with_html(acct, "t-auth").unwrap();
    let got: Vec<Option<bool>> = view.messages.iter().map(|m| m.auth_pass).collect();
    assert_eq!(got, vec![None, Some(true), Some(false)]);

    // A re-sync of the NULL row refills the column through the upsert's DO
    // UPDATE half — which is why no backfill is needed.
    triaged(acct, "g-null", "t-auth")
        .received_at(at(1))
        .auth_pass(Some(true))
        .upsert(&store);
    let view = store.thread_view_with_html(acct, "t-auth").unwrap();
    assert_eq!(view.messages.len(), 3, "re-upsert is still one row");
    assert_eq!(view.messages[0].auth_pass, Some(true));
}

#[test]
fn thread_view_carries_is_sent_per_message() {
    // The reader right-aligns on this bit, so a mixed thread must answer it
    // message by message, not thread-wide.
    let (store, acct) = store();
    let at = |n: i64| Utc::now() - chrono::Duration::minutes(10 - n);

    triaged(acct, "g-in", "t-sent")
        .received_at(at(1))
        .upsert(&store);
    triaged(acct, "g-out", "t-sent")
        .received_at(at(2))
        .from("me@example.com")
        .is_sent(true)
        .to_addrs("Alice <alice@example.com>")
        .upsert(&store);

    let view = store.thread_view_with_html(acct, "t-sent").unwrap();
    let got: Vec<bool> = view.messages.iter().map(|m| m.is_sent).collect();
    assert_eq!(
        got,
        vec![false, true],
        "oldest-first: received, then the reply"
    );
}

#[test]
fn thread_view_is_sent_is_authorship_not_the_sticky_stored_flag() {
    // The stored column is a VISIBILITY flag: `MIN` on conflict pins it to 0
    // once anything has seen the message as received, and the sync engine
    // deliberately lets the INBOX copy of self-addressed mail win. So a message
    // the user demonstrably WROTE can sit at is_sent=0 forever, and the served
    // bit has to come from authorship — From == the account's own address,
    // case-folded — or the reader draws the user's own words on the far side.
    let (store, acct) = store();
    let at = |n: i64| Utc::now() - chrono::Duration::minutes(10 - n);

    // Self-Cc'd: the INBOX walk ingested it first (is_sent=0), then the SENT
    // copy landed, and the sticky clause refused the flip. The From header
    // spells the account's address in a different case than the accounts row.
    triaged(acct, "g-selfcc", "t-self")
        .received_at(at(1))
        .from("Me@Example.COM")
        .upsert(&store);
    let self_id = triaged(acct, "g-selfcc", "t-self")
        .received_at(at(1))
        .from("Me@Example.COM")
        .is_sent(true)
        .upsert(&store);
    // A stranger's reply in the same thread, and a row whose sender never
    // parsed — the blank From falls back to the stored flag rather than
    // matching anything.
    triaged(acct, "g-them", "t-self")
        .received_at(at(2))
        .from("alice@example.com")
        .upsert(&store);
    triaged(acct, "g-blank", "t-self")
        .received_at(at(3))
        .from("")
        .upsert(&store);

    let stored: i64 = {
        let conn = store.lock().unwrap();
        conn.query_row(
            "SELECT is_sent FROM messages WHERE id = ?1",
            params![self_id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(stored, 0, "the stored flag is sticky to 0, by design");

    let view = store.thread_view_with_html(acct, "t-self").unwrap();
    let got: Vec<bool> = view.messages.iter().map(|m| m.is_sent).collect();
    assert_eq!(
        got,
        vec![true, false, false],
        "the user's own self-Cc'd message is theirs; the stranger's and the \
         senderless row are not"
    );
}

#[test]
fn the_thread_view_says_which_messages_are_the_users_own() {
    // The reader aims its actions — remind me about this, reply to this — at
    // INBOUND mail, and a conversation ends on the user's own reply often
    // enough that "the last message" would aim at themselves. So `is_sent`
    // rides on every message, always present on the wire.
    let (store, acct) = store();
    let at = |n: i64| Utc::now() - chrono::Duration::minutes(10 - n);
    triaged(acct, "g-in", "t-mine")
        .received_at(at(1))
        .seed(&store);
    triaged(acct, "g-out", "t-mine")
        .received_at(at(2))
        .from("me@example.com")
        .is_sent(true)
        .upsert(&store);

    let view = store.thread_view_with_html(acct, "t-mine").unwrap();
    assert_eq!(
        view.messages.iter().map(|m| m.is_sent).collect::<Vec<_>>(),
        vec![false, true]
    );
    // A plain bool, never absent: an old client decoding it gets `false`, which
    // is the safe reading (aim at it) rather than a missing key.
    let wire = serde_json::to_value(&view.messages[0]).unwrap();
    assert_eq!(wire["is_sent"], serde_json::Value::Bool(false));
}

#[test]
fn human_attachment_bytes_allow_restricted_parents_and_guard_size_and_ownership() {
    let (store, acct) = store();

    // Normal parent with a stored attachment and an over-cap (NULL data) one.
    let mid = triaged(acct, "g1", "t1").importance(10).seed(&store);
    let a_ok = store
        .insert_attachment(acct, mid, "doc.pdf", "application/pdf", 5, Some(b"Hello"))
        .unwrap();
    let a_over = store
        .insert_attachment(
            acct,
            mid,
            "big.bin",
            "application/octet-stream",
            11_000_000,
            None,
        )
        .unwrap();

    // Normal parent, bytes present.
    let ok = store.attachment_bytes(acct, a_ok).unwrap().expect("row");
    assert_eq!(ok.2.as_deref(), Some(&b"Hello"[..]));

    // Over-cap: row resolves but data is None (endpoint -> 410).
    let over = store
        .attachment_bytes(acct, a_over)
        .unwrap()
        .expect("metadata row exists");
    assert!(over.2.is_none(), "over-cap attachment carries no bytes");

    // Unknown id -> None (endpoint -> 404).
    assert!(store.attachment_bytes(acct, 999_999).unwrap().is_none());

    // Restricted external access does not prevent the human reading attachments.
    let sid = triaged(acct, "g2", "t2")
        .sealed(SealedKind::Otp)
        .seed(&store);
    let sealed_att = store
        .insert_attachment(
            acct,
            sid,
            "secret.pdf",
            "application/pdf",
            6,
            Some(b"secret"),
        )
        .unwrap();
    assess_external(&store, acct, sid, "restricted");
    let attachment = store.attachment_bytes(acct, sealed_att).unwrap().unwrap();
    assert_eq!(attachment.2.as_deref(), Some(&b"secret"[..]));
    let other = store.ensure_account("other@example.com").unwrap();
    assert!(store.attachment_bytes(other, sealed_att).unwrap().is_none());
}

#[test]
fn field_reasons_roundtrip_through_ingest_and_attention_updates() {
    use crate::types::FieldReasons;
    let (store, acct) = store();

    // Build a normal inbound TriagedMessage carrying per-property reasons.
    let id = inbound_triaged(acct, "g1", "t1", "boss@work.com", Utc::now(), false)
        .importance(72)
        .tier(Tier::Signal)
        .reason("known contact")
        .field_reasons(FieldReasons {
            importance: Some("known contact -> signal importance 72".into()),
            deadline: None,
            tier: Some("known contact -> signal".into()),
        })
        .ingest(&store);

    // HUMAN DOOR: attention_updates carries the parsed field_reasons.
    let ups = store
        .attention_updates(
            acct,
            Utc::now() - chrono::Duration::days(1),
            None,
            None,
            None,
            false,
            SpamScope::Exclude,
        )
        .unwrap();
    let u = ups.iter().find(|u| u.update.id == id).expect("row present");
    let fr = u
        .update
        .field_reasons
        .as_ref()
        .expect("field_reasons present");
    assert_eq!(
        fr.importance.as_deref(),
        Some("known contact -> signal importance 72")
    );
    assert_eq!(fr.tier.as_deref(), Some("known contact -> signal"));
    assert!(fr.deadline.is_none());
    // And it serializes into the /client/updates JSON as an object.
    let v = serde_json::to_value(&u.update).unwrap();
    assert_eq!(
        v["field_reasons"]["tier"],
        serde_json::json!("known contact -> signal")
    );

    // AGENT DOOR: ranked_updates (MCP) never carries field_reasons — the key
    // is absent from the serialized Update.
    let ranked = store
        .ranked_updates(acct, Utc::now() - chrono::Duration::days(1), None)
        .unwrap();
    let r = ranked.iter().find(|u| u.id == id).expect("row present");
    assert!(r.field_reasons.is_none());
    let rv = serde_json::to_value(r).unwrap();
    assert!(
        rv.get("field_reasons").is_none(),
        "MCP payload must omit field_reasons: {rv}"
    );
    // Same byte-absence discipline for the paperclip flag.
    assert!(r.has_attachments.is_none());
    assert!(
        rv.get("has_attachments").is_none(),
        "MCP payload must omit has_attachments: {rv}"
    );
}

#[test]
fn predating_triage_row_reads_back_as_none() {
    // A row written with no field_reasons (NULL column) reads back as None.
    let (store, acct) = store();
    let mid = triaged(acct, "g1", "t1")
        .importance(60)
        .tier(Tier::Signal)
        .one_line("x")
        .reason("y")
        .seed(&store);
    let ups = store
        .attention_updates(
            acct,
            Utc::now() - chrono::Duration::days(1),
            None,
            None,
            None,
            false,
            SpamScope::Exclude,
        )
        .unwrap();
    let u = ups.iter().find(|u| u.update.id == mid).unwrap();
    assert!(u.update.field_reasons.is_none());
}

#[test]
fn round_trips_a_message() {
    let (store, acct) = store();
    let id = triaged(acct, "g1", "t1")
        .importance(80)
        .tier(Tier::Signal)
        .one_line("Lunch invite")
        .reason("known contact")
        .seed(&store);

    let updates = store
        .ranked_updates(acct, Utc::now() - chrono::Duration::days(1), Some(1))
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].sender, "alice@example.com");
    assert_eq!(updates[0].tier, Tier::Signal);

    assess_external(&store, acct, id, "allowed");
    let tv = store.thread_view(acct, "t1").unwrap();
    assert_eq!(tv.messages.len(), 1);
    assert_eq!(tv.subject, "Lunch?");
}

/// `thread_id_for_message` (the get_thread forgiveness fallback) resolves a
/// normal message id to its thread, returns None for an unknown id, and
/// returns None for a SEALED message id — so a sealed id is indistinguishable
/// from a nonexistent one and never leaks thread existence.
#[test]
fn thread_id_for_message_resolves_normal_and_hides_sealed() {
    let (store, acct) = store();

    let normal = triaged(acct, "g1", "t1")
        .importance(80)
        .tier(Tier::Signal)
        .seed(&store);
    let sealed = triaged(acct, "g2", "t2")
        .importance(90)
        .sealed(SealedKind::Otp)
        .seed(&store);

    assess_external(&store, acct, normal, "allowed");
    assess_external(&store, acct, sealed, "restricted");
    let pending = triaged(acct, "pending", "pending").seed(&store);
    assert_eq!(store.thread_id_for_message(acct, pending).unwrap(), None);
    assert_eq!(
        store
            .thread_id_for_message(acct, normal)
            .unwrap()
            .as_deref(),
        Some("t1")
    );
    assert_eq!(store.thread_id_for_message(acct, 999_999).unwrap(), None);
    assert_eq!(
        store.thread_id_for_message(acct, sealed).unwrap(),
        None,
        "sealed message id must not resolve (no thread-existence leak)"
    );
}

#[test]
fn restricted_mail_is_readable_by_humans_but_not_external_agents() {
    let (store, acct) = store();

    // A normal message.
    let allowed = triaged(acct, "g1", "t1")
        .importance(80)
        .tier(Tier::Signal)
        .one_line("Lunch")
        .seed(&store);

    // A sealed OTP message in a different thread.
    let restricted = triaged(acct, "g2", "t2")
        .subject("Your verification code")
        .from("noreply@bank.com")
        .importance(90)
        .sealed(SealedKind::Otp)
        .one_line("code")
        .reason("otp")
        .seed(&store);

    assess_external(&store, acct, allowed, "allowed");
    assess_external(&store, acct, restricted, "restricted");
    assert!(store.thread_view(acct, "t1").is_ok());
    // Human ranked updates include both rows.
    let updates = store
        .ranked_updates(acct, Utc::now() - chrono::Duration::days(1), None)
        .unwrap();
    assert_eq!(updates.len(), 2);
    assert!(updates.iter().any(|u| u.thread_id == "t2"));

    // thread_view on the sealed thread => NotFound.
    let err = store.thread_view(acct, "t2").unwrap_err();
    assert!(matches!(err, CoreError::NotFound));

    // Nonexistent thread also => NotFound (indistinguishable).
    let err2 = store.thread_view(acct, "does-not-exist").unwrap_err();
    assert!(matches!(err2, CoreError::NotFound));

    // The human can open restricted mail immediately.
    assert!(store.thread_view_with_html(acct, "t2").is_ok());
    assert!(matches!(
        store
            .thread_view_with_html(acct, "does-not-exist")
            .unwrap_err(),
        CoreError::NotFound
    ));

    // Merely restricting external access does not assert canonical auth kinds.
    // The dedicated Auth listing requires the model's auth assessment.
    assert!(store.sealed_messages(acct).unwrap().is_empty());
}

#[test]
fn human_deadlines_include_restricted_source() {
    let (store, acct) = store();
    let mid = triaged(acct, "g1", "t1")
        .importance(50)
        .tier(Tier::Deadline)
        .sensitivity(Sensitivity::Sealed)
        .seed(&store);

    {
        let conn = store.lock().unwrap();
        conn.execute(
            "INSERT INTO deadlines(account_id, message_id, kind, due_at, past_due, source)
             VALUES(?1,?2,'bill',?3,0,'regex')",
            params![
                acct,
                mid,
                (Utc::now() + chrono::Duration::days(2)).to_rfc3339()
            ],
        )
        .unwrap();
    }

    let ds = store.deadlines(acct, Some(30)).unwrap();
    assert_eq!(ds.len(), 1, "human records remain readable");
}

#[test]
fn sealed_body_reveal_audit_and_stats() {
    let (store, acct) = store();

    let s = triaged(acct, "g1", "t1")
        .body("secret 123456")
        .importance(90)
        .sealed(SealedKind::Otp)
        .seed(&store);

    let nid = triaged(acct, "g2", "t2")
        .importance(80)
        .tier(Tier::Signal)
        .seed(&store);

    // sealed_body returns only for the sealed message.
    let body = store.sealed_body(acct, s).unwrap();
    assert_eq!(body.body, "secret 123456");
    assert!(matches!(
        store.sealed_body(acct, nid).unwrap_err(),
        CoreError::NotFound
    ));

    // audit append + list
    let aid = store
        .append_audit(
            acct,
            &crate::store::NewAuditEntry {
                actor: "human".into(),
                action: "reveal_sealed".into(),
                target: Some(s.to_string()),
                detail: None,
            },
        )
        .unwrap();
    assert!(aid > 0);
    let audit = store.list_audit(acct, 10).unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].action, "reveal_sealed");

    // Human inventory contains both; a legacy seal is not canonical auth.
    let stats = store
        .stats(acct, Utc::now() - chrono::Duration::days(30))
        .unwrap();
    assert_eq!(stats.total, 2);
    assert_eq!(stats.tier_counts.get("signal").copied(), Some(1));
    assert_eq!(stats.sealed, 0);
}

#[test]
fn reingest_preserves_llm_classification_but_refreshes_heuristic_rows() {
    let (store, acct) = store();
    let since = Utc::now() - chrono::Duration::days(3650);

    // --- Row A: LLM-classified, then re-delivered. ---
    let a = triaged_row(acct, "g-a", "t-a", None, false, Sensitivity::Normal).ingest(&store);
    // Stage-1 refines it with a REAL model id + distinctive values.
    store
        .stage1_apply(&Stage1Applied {
            message_id: a,
            account_id: acct,
            importance: 88,
            tier: Tier::Signal,
            one_line: "LLM verdict".into(),
            reason: "stage-1 refined".into(),
            field_reasons: crate::types::FieldReasons::default(),
            stage1_model_used: "claude-haiku-4-5".into(),
            needs_stage2: false,
            escalation_reason: None,
            deadline: None,
            category: Some("general".into()),
        })
        .unwrap();
    // Re-deliver the SAME message (heuristic seed carries importance 40).
    triaged_row(acct, "g-a", "t-a", None, false, Sensitivity::Normal).ingest(&store);
    let ups = store.ranked_updates(acct, since, None).unwrap();
    let ua = ups.iter().find(|u| u.id == a).expect("row A present");
    assert_eq!(
        ua.importance, 88,
        "paid LLM importance preserved on re-ingest"
    );
    assert_eq!(ua.one_line, "LLM verdict", "paid LLM one_line preserved");
    assert_eq!(ua.tier, Tier::Signal, "paid LLM tier preserved");

    // --- Row B: still heuristic-only -> re-ingest refreshes the seed. ---
    let b = triaged_row(acct, "g-b", "t-b", None, false, Sensitivity::Normal).ingest(&store);
    triaged_row(acct, "g-b", "t-b", None, false, Sensitivity::Normal)
        .importance(71)
        .tier(Tier::Signal)
        .one_line("fresh seed")
        .ingest(&store);
    let ups = store.ranked_updates(acct, since, None).unwrap();
    let ub = ups.iter().find(|u| u.id == b).expect("row B present");
    assert_eq!(ub.importance, 71, "still-heuristic row adopts the new seed");
    assert_eq!(ub.one_line, "fresh seed");
}

/// A RE-INGEST MAY NOT UNDO A PERSON'S SEAL, and this is the one verdict column
/// where that was not true. A re-ingest carries only heuristic SEED values, and
/// the detector never saw whatever made the user call this mail auth, so the old
/// unconditional `sensitivity=excluded.sensitivity` reverted the seal on any
/// re-walk inside the freshness window — a catch-up, or the routine one a held
/// cursor causes.
///
/// It is not a bookkeeping loss. An unsealed row is a row the fast lane will
/// send to the notify model and turn into an `events` row minted AFTER
/// `correct_triage` ran its redaction, so there is nothing left that can redact
/// it, and it replays over SSE to every cursor forever (docs/SECURITY.md §4).
#[test]
fn reingest_cannot_revert_a_human_seal_but_a_detector_seal_still_refreshes() {
    let (store, acct) = store();
    let sealed_now = |mid: i64| -> String {
        let conn = store.lock().unwrap();
        conn.query_row(
            "SELECT sensitivity FROM triage WHERE message_id=?1",
            params![mid],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };

    // --- Row A: a person seals a message the detector called normal. ---
    let a = triaged_row(acct, "g-a", "t-a", None, false, Sensitivity::Normal).ingest(&store);
    store
        .correct_triage(
            acct,
            a,
            crate::types::TriageAxis::Sensitivity,
            "sealed",
            None,
            Utc::now(),
        )
        .unwrap();
    // The same gmail id walked again, carrying the seed's `normal` verdict.
    triaged_row(acct, "g-a", "t-a", None, false, Sensitivity::Normal).ingest(&store);
    assert_eq!(
        sealed_now(a),
        "sealed",
        "only a person outranks a person: the seed cannot un-seal what a human sealed"
    );

    // --- Row B: no human ever touched it, so the fresh detection still wins,
    // which is what lets a detector FIX newly seal an old message. ---
    let b = triaged_row(acct, "g-b", "t-b", None, false, Sensitivity::Normal).ingest(&store);
    assert_eq!(sealed_now(b), "normal");
    triaged_row(acct, "g-b", "t-b", None, false, Sensitivity::Sealed).ingest(&store);
    assert_eq!(
        sealed_now(b),
        "sealed",
        "an improved detector must still be able to seal a row it missed"
    );

    // --- Row C: a person corrected the TIER, which has nothing to say about
    // whether this is auth mail. `correct_triage` stamps `model_used='human'`
    // for EVERY axis, so a freeze keyed on that column would pin row C's
    // sensitivity forever and quietly delete row B's behaviour for every
    // human-corrected row in the mailbox. The freeze is keyed on the AXIS the
    // person actually ruled on. ---
    let c = triaged_row(acct, "g-c", "t-c", None, false, Sensitivity::Normal).ingest(&store);
    store
        .correct_triage(
            acct,
            c,
            crate::types::TriageAxis::Tier,
            "signal",
            None,
            Utc::now(),
        )
        .unwrap();
    triaged_row(acct, "g-c", "t-c", None, false, Sensitivity::Sealed).ingest(&store);
    assert_eq!(
        sealed_now(c),
        "sealed",
        "a tier correction is not a ruling about auth mail, and must not stop \
         a detector fix from sealing this row"
    );

    // --- Row D: the OTHER direction of a sensitivity ruling. The detector is
    // recall-biased and over-seals; a person saying "this is ordinary mail"
    // must outrank the same detector on the next re-walk, or the correction is
    // one the user makes again every poll — and the fast lane pings it as a
    // login code each time (docs/NOTIFY.md §11.6). ---
    let d = triaged_row(acct, "g-d", "t-d", None, false, Sensitivity::Sealed).ingest(&store);
    store
        .correct_triage(
            acct,
            d,
            crate::types::TriageAxis::Sensitivity,
            "normal",
            None,
            Utc::now(),
        )
        .unwrap();
    triaged_row(acct, "g-d", "t-d", None, false, Sensitivity::Sealed).ingest(&store);
    assert_eq!(
        sealed_now(d),
        "normal",
        "only a person outranks a person, in the un-seal direction too"
    );
}

#[test]
fn reingest_preserves_a_processed_ship_marker_but_refreshes_pending() {
    let (store, acct) = store();
    let marker = |mid: i64| -> Option<String> {
        let conn = store.lock().unwrap();
        conn.query_row(
            "SELECT ship_extract_model FROM triage WHERE message_id=?1",
            params![mid],
            |r| r.get(0),
        )
        .unwrap()
    };

    // --- Row A: PROCESSED by the shipments extractor, then re-delivered. ---
    let a = triaged_row(acct, "g-a", "t-a", None, false, Sensitivity::Normal)
        .ship_extract(true)
        .ingest(&store);
    store
        .ship_extract_mark(acct, a, "claude-haiku-4-5")
        .unwrap();
    triaged_row(acct, "g-a", "t-a", None, false, Sensitivity::Normal)
        .ship_extract(true)
        .ingest(&store);
    assert_eq!(
        marker(a).as_deref(),
        Some("claude-haiku-4-5"),
        "a paid verdict is not re-spent on the same mail"
    );

    // --- Row B: still 'pending' -> the fresh detection wins. Here the signal
    //     goes AWAY (a detector fix), and the row must leave the queue. ---
    let b = triaged_row(acct, "g-b", "t-b", None, false, Sensitivity::Normal)
        .ship_extract(true)
        .ingest(&store);
    assert_eq!(marker(b).as_deref(), Some("pending"));
    triaged_row(acct, "g-b", "t-b", None, false, Sensitivity::Normal).ingest(&store);
    assert_eq!(marker(b), None, "a still-pending trigger refreshes");

    // --- Row C: no signal at first, then one -> newly queued. ---
    let c = triaged_row(acct, "g-c", "t-c", None, false, Sensitivity::Normal).ingest(&store);
    assert_eq!(marker(c), None);
    triaged_row(acct, "g-c", "t-c", None, false, Sensitivity::Normal)
        .ship_extract(true)
        .ingest(&store);
    assert_eq!(marker(c).as_deref(), Some("pending"));
}

#[test]
fn a_received_sighting_pins_is_sent_to_zero_across_re_upserts() {
    // is_sent=1 removes a message from every listing surface, so one
    // mislabeled SENT sighting flipping a received row would vanish inbound
    // mail. The upsert's MIN() makes the received bit sticky no matter which
    // copy lands first; only sent-only mail stays sent.
    let (store, acct) = store();
    let is_sent_of = |id: i64| -> i64 {
        store
            .lock()
            .unwrap()
            .query_row("SELECT is_sent FROM messages WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    };

    // Inbound first, mislabeled SENT copy second: the flip must not take.
    let id = triaged(acct, "g-flip", "t-flip").upsert(&store);
    triaged(acct, "g-flip", "t-flip")
        .is_sent(true)
        .upsert(&store);
    assert_eq!(is_sent_of(id), 0, "a received row never flips to sent");

    // Sent first (the api echo path), inbox copy second: visibility is gained.
    let id2 = triaged(acct, "g-gain", "t-gain")
        .is_sent(true)
        .upsert(&store);
    triaged(acct, "g-gain", "t-gain").upsert(&store);
    assert_eq!(is_sent_of(id2), 0, "an inbox sighting always wins");

    // Sent-only mail stays sent across re-upserts.
    let id3 = triaged(acct, "g-out", "t-out").is_sent(true).upsert(&store);
    triaged(acct, "g-out", "t-out").is_sent(true).upsert(&store);
    assert_eq!(is_sent_of(id3), 1, "sent-only mail stays sent");
}

// ---- the sent listing (human door) --------------------------------------

/// A sent message with its display recipients and a seeded triage row — what
/// every real ingest of outbound mail lands.
fn sent(
    store: &SqliteStore,
    acct: AccountId,
    gmail: &str,
    to: &str,
    received: DateTime<Utc>,
) -> i64 {
    triaged(acct, gmail, &format!("t-{gmail}"))
        .is_sent(true)
        .to_addrs(to)
        .subject("Re: Lunch?")
        .received_at(received)
        .seed(store)
}

#[test]
fn sent_listing_shows_only_sent_mail_newest_first_with_recipients_and_opens() {
    let (store, acct) = store();
    let t0 = Utc::now() - chrono::Duration::days(3);

    // Two sent messages a day apart, plus ordinary inbound mail.
    let older = sent(&store, acct, "s-old", "Alice <alice@friends.com>", t0);
    let newer = sent(
        &store,
        acct,
        "s-new",
        "Bob <bob@friends.com>, carol@friends.com",
        t0 + chrono::Duration::days(1),
    );
    triaged(acct, "g-in", "t-in")
        .received_at(t0 + chrono::Duration::days(2))
        .seed(&store);

    // One recorded open against the newer message's tracker.
    store
        .insert_send_tracker(acct, "tok-1", None, 1_000)
        .unwrap();
    assert!(
        store
            .set_send_tracker_message(acct, "tok-1", newer)
            .unwrap()
    );
    assert!(
        store
            .record_open(acct, "tok-1", 1_100, Some("Apple Mail/16.0"), "unknown")
            .unwrap()
    );

    let rows = store.sent_listing(acct, 50, 0).unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        vec![newer, older],
        "sent mail only, newest first — inbound mail is never listed here"
    );
    assert_eq!(rows[0].to, "Bob <bob@friends.com>, carol@friends.com");
    assert_eq!(rows[0].opens, 1, "read receipts ride along");
    assert_eq!(rows[1].to, "Alice <alice@friends.com>");
    assert_eq!(rows[1].opens, 0, "an untracked send has no opens");
    assert_eq!(rows[0].thread_id, "t-s-new");
    assert_eq!(rows[0].subject, "Re: Lunch?");
    assert_eq!(
        rows[0].sent_at,
        (t0 + chrono::Duration::days(1)).to_rfc3339(),
        "received_at is served verbatim"
    );

    // Paging is the same offset window every other listing uses.
    let page = store.sent_listing(acct, 1, 0).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, newer);
    let next = store.sent_listing(acct, 1, 1).unwrap();
    assert_eq!(next.len(), 1);
    assert_eq!(
        next[0].id, older,
        "no row is dropped or repeated across pages"
    );
}

#[test]
fn human_sent_listing_includes_pending_and_restricted_rows() {
    let (store, acct) = store();
    let now = Utc::now();

    triaged(acct, "s-sealed", "t-sealed")
        .is_sent(true)
        .to_addrs("support@bank.com")
        .received_at(now)
        .sealed(SealedKind::Otp)
        .seed(&store);

    triaged(acct, "s-orphan", "t-orphan")
        .is_sent(true)
        .to_addrs("alice@friends.com")
        .received_at(now)
        .upsert(&store);

    triaged(acct, "g-seal-sibling", "t-mixed")
        .sealed(SealedKind::Otp)
        .received_at(now)
        .seed(&store);
    triaged(acct, "s-in-sealed-thread", "t-mixed")
        .is_sent(true)
        .to_addrs("alice@friends.com")
        .received_at(now)
        .seed(&store);

    let _ok = sent(&store, acct, "s-ok", "alice@friends.com", now);
    let rows = store.sent_listing(acct, 50, 0).unwrap();
    assert_eq!(rows.len(), 4, "all four sent copies remain human-readable");
}

#[test]
fn sent_listing_is_account_scoped_and_reads_missing_recipients_as_empty() {
    let (store, acct) = store();
    let theirs = store.ensure_account("other@example.com").unwrap();
    let now = Utc::now();

    // A pre-backfill row: is_sent with to_addrs still NULL.
    let bare = triaged(acct, "s-bare", "t-bare")
        .is_sent(true)
        .received_at(now)
        .seed(&store);
    sent(&store, theirs, "s-theirs", "them@elsewhere.com", now);

    let rows = store.sent_listing(acct, 50, 0).unwrap();
    assert_eq!(rows.len(), 1, "another account's sent mail is not visible");
    assert_eq!(rows[0].id, bare);
    assert_eq!(rows[0].to, "", "NULL recipients read as empty, never NULL");
}

// ---- the recipients backfill queue --------------------------------------

#[test]
fn recipients_backfill_queue_drains_as_rows_are_filled() {
    let (store, acct) = store();
    let now = Utc::now();

    let pending = triaged(acct, "s-pending", "t-pending")
        .is_sent(true)
        .received_at(now)
        .seed(&store);
    // Already filled, and received mail: neither is ever in the queue.
    sent(&store, acct, "s-filled", "alice@friends.com", now);
    triaged(acct, "g-in", "t-in").received_at(now).seed(&store);

    let queue = store.sent_missing_recipients(acct, 50).unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].message_id, pending);
    assert_eq!(queue[0].gmail_msg_id, "s-pending");

    // Writing "" ("looked, nobody named") is what takes a row OUT of the queue,
    // so one headerless message cannot re-queue the pass forever.
    assert!(store.set_message_to_addrs(acct, pending, "").unwrap());
    assert!(store.sent_missing_recipients(acct, 50).unwrap().is_empty());
    assert_eq!(store.sent_listing(acct, 50, 0).unwrap().len(), 2);
}

#[test]
fn set_message_to_addrs_refuses_received_mail() {
    let (store, acct) = store();
    let inbound = triaged(acct, "g-in", "t-in").seed(&store);
    assert!(
        !store
            .set_message_to_addrs(acct, inbound, "someone@example.com")
            .unwrap(),
        "recipients are a property of mail the user SENT"
    );
    let stored: Option<String> = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT to_addrs FROM messages WHERE id=?1",
            [inbound],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, None);
}

#[test]
fn upsert_keeps_stored_recipients_when_a_later_write_has_none() {
    // Only the sent path parses recipients, so a writer with no opinion must
    // not blank a column the backfill (or an earlier ingest) already filled.
    let (store, acct) = store();
    let id = triaged(acct, "s-keep", "t-keep")
        .is_sent(true)
        .to_addrs("Alice <alice@friends.com>")
        .upsert(&store);
    triaged(acct, "s-keep", "t-keep")
        .is_sent(true)
        .upsert(&store);

    let stored: Option<String> = store
        .lock()
        .unwrap()
        .query_row("SELECT to_addrs FROM messages WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(stored.as_deref(), Some("Alice <alice@friends.com>"));
}

#[test]
fn inbox_unread_counts_round_trip_and_overwrite_one_row() {
    // The human door serves absence differently from zero, so the never-fetched
    // case must be None and a real zero must be Some(0) — this is the whole
    // reason the counts are their own row instead of a defaulted column.
    let (store, acct) = store();
    assert!(
        store.inbox_unread(acct).unwrap().is_none(),
        "never fetched reads as absence"
    );

    let before = Utc::now();
    store.set_inbox_unread(acct, 214, 190).unwrap();
    let got = store.inbox_unread(acct).unwrap().expect("counts stored");
    assert_eq!((got.messages, got.threads), (214, 190));
    assert!(got.fetched_at >= before, "fetched_at is stamped at write");

    // A later fetch OVERWRITES: this mirrors Gmail's current state, so a second
    // row would be a second answer to a question with one answer.
    store.set_inbox_unread(acct, 0, 0).unwrap();
    let got = store
        .inbox_unread(acct)
        .unwrap()
        .expect("zero is an answer");
    assert_eq!((got.messages, got.threads), (0, 0));
    let rows: i64 = store
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM inbox_unread", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "one row per account, overwritten in place");

    // Per account: another mailbox's counts are not this one's.
    let other = store.ensure_account("other@example.com").unwrap();
    assert!(store.inbox_unread(other).unwrap().is_none());
}

#[test]
fn external_access_handles_shared_source_cycles_and_restriction_changes() {
    let (store, acct) = store();
    let a = triaged(acct, "cycle-a", "cycle").seed(&store);
    let b = triaged(acct, "cycle-b", "cycle").seed(&store);
    assess_external(&store, acct, a, "allowed");
    assess_external(&store, acct, b, "allowed");
    {
        let conn = store.lock().unwrap();
        for (target, source) in [(a, b), (b, a)] {
            conn.execute(
                "INSERT INTO agent_decision_sources(account_id,message_id,source_message_id,source_revision)
                 SELECT ?1,?2,?3,revision FROM agent_message_state WHERE account_id=?1 AND message_id=?3",
                params![acct, target, source],
            ).unwrap();
        }
    }
    assert!(
        store.thread_view(acct, "cycle").is_ok(),
        "ordinary mutually consumed thread context stays readable"
    );
    assess_external(&store, acct, b, "restricted");
    assert!(!store.external_thread_allowed(acct, "cycle").unwrap());
    assess_external(&store, acct, b, "allowed");
    assert!(store.external_thread_allowed(acct, "cycle").unwrap());
    store.lock().unwrap().execute(
        "UPDATE agent_message_state SET revision=revision+1 WHERE account_id=?1 AND message_id=?2",
        params![acct, b],
    ).unwrap();
    assert!(
        !store.external_thread_allowed(acct, "cycle").unwrap(),
        "a stale consumed source blocks derived content"
    );
}

#[test]
fn model_access_assessment_replaces_legacy_auth_sealing() {
    let (store, acct) = store();
    let id = triaged(acct, "old-login", "login")
        .sealed(SealedKind::LoginAlert)
        .seed(&store);
    assert!(
        store.thread_view(acct, "login").is_err(),
        "no new assessment yet"
    );
    assess_external(&store, acct, id, "allowed");
    assert!(store.thread_view(acct, "login").is_ok());
    assert_eq!(
        store.thread_id_for_message(acct, id).unwrap().as_deref(),
        Some("login")
    );
}

#[test]
fn external_shipments_require_all_field_provenance_to_be_allowed() {
    let (store, acct) = store();
    let origin = triaged(acct, "shipment", "shipment").seed(&store);
    let name = triaged(acct, "name", "name").seed(&store);
    assess_external(&store, acct, origin, "allowed");
    assess_external(&store, acct, name, "restricted");
    let id = store
        .upsert_shipment(
            acct,
            origin,
            &crate::triage::ShipmentInfo {
                carrier: "ups".into(),
                tracking_number: "1Z999AA10123456784".into(),
                item_name: "Package".into(),
                status: crate::triage::ShipmentStatus::Shipped,
                tracking_url: None,
            },
            Utc::now(),
        )
        .unwrap();
    assert!(store.external_shipment_allowed(acct, id).unwrap());
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE shipments SET item_name_msg=?1 WHERE account_id=?2 AND id=?3",
            params![name, acct, id],
        )
        .unwrap();
    assert!(!store.external_shipment_allowed(acct, id).unwrap());
    assess_external(&store, acct, name, "allowed");
    assert!(store.external_shipment_allowed(acct, id).unwrap());
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE shipments SET created_by_message_id=NULL WHERE account_id=?1 AND id=?2",
            params![acct, id],
        )
        .unwrap();
    assert!(
        !store.external_shipment_allowed(acct, id).unwrap(),
        "missing legacy provenance is not permission"
    );
    assert!(!store.external_shipment_allowed(acct + 100, id).unwrap());
}

#[test]
fn shredder_requires_current_actionable_auth_assessment() {
    let (store, acct) = store();
    let old = Utc::now() - chrono::Duration::days(10);
    triaged(acct, "old-regex", "old-regex-thread")
        .received_at(old)
        .sealed(SealedKind::Otp)
        .seed(&store);
    let actionable = triaged(acct, "auth", "auth-thread")
        .received_at(old)
        .seed(&store);
    let login = triaged(acct, "login", "login-thread")
        .received_at(old)
        .seed(&store);
    let stale = triaged(acct, "stale", "stale-thread")
        .received_at(old)
        .seed(&store);
    let allowed = triaged(acct, "allowed", "allowed-thread")
        .received_at(old)
        .seed(&store);
    for (id, kind, access) in [
        (actionable, "otp", "restricted"),
        (login, "login_alert", "restricted"),
        (stale, "password_reset", "restricted"),
        (allowed, "otp", "allowed"),
    ] {
        assess_external(&store, acct, id, access);
        store.lock().unwrap().execute(
            "INSERT INTO agent_message_decisions(account_id,message_id,revision,decision_json,decided_at)
             SELECT account_id,message_id,revision,?3,?4 FROM agent_message_state WHERE account_id=?1 AND message_id=?2",
            params![acct,id,serde_json::json!({"auth":{"kinds":[kind]}}).to_string(),Utc::now().to_rfc3339()],
        ).unwrap();
    }
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE agent_message_state SET revision=revision+1 WHERE message_id=?1",
            params![stale],
        )
        .unwrap();
    let cutoff = Utc::now() - chrono::Duration::days(1);
    let candidates = store.shred_candidates(acct, cutoff, 100).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].message_id, actionable);
    assert_eq!(candidates[0].kind.as_deref(), Some("otp"));
    assert_eq!(store.shred_pending_count(acct, cutoff).unwrap(), 1);
    assert_eq!(store.shred_pending_count(acct + 1, cutoff).unwrap(), 0);
    assert_eq!(
        store
            .stats(acct, old - chrono::Duration::days(1))
            .unwrap()
            .sealed,
        1
    );
    store
        .record_shred(acct, &candidates[0], Utc::now())
        .unwrap();
    assert_eq!(store.shred_pending_count(acct, cutoff).unwrap(), 0);
}

#[test]
fn external_thread_read_demand_queues_bounded_legacy_access_work() {
    let (store, acct) = store();
    for i in 0..20 {
        triaged(acct, &format!("ancestor-{i}"), "legacy-thread")
            .received_at(Utc::now() - chrono::Duration::days(60))
            .seed(&store);
    }
    let count = || -> i64 {
        store.lock().unwrap().query_row(
        "SELECT COUNT(*) FROM agent_triage_jobs WHERE account_id=?1 AND kind='access' AND trigger='source_access' AND arrival_eligible=0",
        params![acct],|r|r.get(0)).unwrap()
    };
    assert_eq!(count(), 0);
    assert!(store.thread_view(acct + 1, "legacy-thread").is_err());
    assert_eq!(count(), 0);
    for expected in [8, 16, 20, 20] {
        assert!(store.thread_view(acct, "legacy-thread").is_err());
        assert_eq!(count(), expected);
    }
    store.lock().unwrap().execute(
        "UPDATE agent_triage_jobs SET state='failed',attempts=6 WHERE account_id=?1 AND kind='access'",
        params![acct],
    ).unwrap();
    assert!(store.thread_view(acct, "legacy-thread").is_err());
    let failures: i64=store.lock().unwrap().query_row(
        "SELECT COUNT(*) FROM agent_triage_jobs WHERE account_id=?1 AND state='failed' AND attempts=6",
        params![acct],|r|r.get(0),
    ).unwrap();
    assert_eq!(
        failures, 20,
        "read retries never reset an exhausted assessment"
    );
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE agent_message_state SET access='allowed' WHERE account_id=?1",
            params![acct],
        )
        .unwrap();
    assert_eq!(
        store
            .thread_view(acct, "legacy-thread")
            .unwrap()
            .messages
            .len(),
        20
    );
    assert_eq!(count(), 20, "readable sources never requeue");
}
