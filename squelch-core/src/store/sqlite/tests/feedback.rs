//! Direct corrections preserve human data and only override the selected field.
use super::super::*;
use super::support::*;
use crate::store::agent_triage::AgentTriageStore;

#[test]
fn corrections_are_audited_and_do_not_freeze_unrelated_reasoning() {
    let (store, account) = store();
    let now = Utc::now();
    let id = inbound_triaged(account, "m", "thread", "sender@test", now, false).ingest(&store);
    let feedback = store
        .correct_triage(
            account,
            id,
            TriageAxis::Category,
            "invoice",
            Some("payment request"),
            now,
        )
        .unwrap()
        .unwrap();
    assert_eq!(feedback.dimension, "category");
    assert_eq!(feedback.note.as_deref(), Some("payment request"));
    let context = store.agent_thread_context(account, "thread").unwrap();
    assert_eq!(context.corrections.len(), 1);
    assert_eq!(context.corrections[0]["field"], "kinds");
    assert_eq!(context.corrections[0]["value"], serde_json::json!(["bill"]));
    assert!(
        store
            .claim_agent_job(account, "triage", now + chrono::Duration::seconds(1), 60)
            .unwrap()
            .is_some()
    );
    assert_eq!(store.list_triage_feedback(account, 10).unwrap().len(), 1);
}

#[test]
fn access_correction_preserves_human_records_and_drafts() {
    let (store, account) = store();
    let now = Utc::now();
    let id = inbound_triaged(account, "m", "thread", "sender@test", now, false).ingest(&store);
    store
        .marketing_apply(&crate::store::MarketingApplied {
            message_id: id,
            account_id: account,
            brand: Some("Shop".into()),
            offer: Some("Sale".into()),
            discount: None,
            code: None,
            expires_at: None,
            received_at: now,
            extractor_model_used: "test".into(),
        })
        .unwrap();
    store.lock().unwrap().execute(
        "INSERT INTO drafts(account_id,reply_to_message_id,body,created_at,updated_at) VALUES(?1,?2,'my reply',?3,?3)",
        params![account,id,now.to_rfc3339()],
    ).unwrap();
    store
        .correct_triage(account, id, TriageAxis::Sensitivity, "sealed", None, now)
        .unwrap();
    assert!(!store.agent_access_allowed(account, id).unwrap());
    let conn = store.lock().unwrap();
    let retained: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM marketing WHERE account_id=?1 AND message_id=?2",
            params![account, id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(retained, 1, "external access does not delete human records");
    let drafts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM drafts WHERE account_id=?1 AND reply_to_message_id=?2",
            params![account, id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(drafts, 1, "external access does not discard human drafts");
    drop(conn);
    store
        .correct_triage(
            account,
            id,
            TriageAxis::Sensitivity,
            "normal",
            None,
            now + chrono::Duration::seconds(1),
        )
        .unwrap();
    assert!(store.agent_access_allowed(account, id).unwrap());
}

#[test]
fn triage_debug_carries_the_thread_id() {
    // The debug read joins `messages` already; the thread id rides along so a
    // client holding one triage row can ask for the whole conversation without
    // a second lookup.
    let (store, acct) = store();
    let t0 = Utc::now();
    let id = inbound_triaged(acct, "g1", "thread-abc", "alice@x.com", t0, false).ingest(&store);

    let debug = store.triage_debug(acct, id).unwrap().expect("triage row");
    assert_eq!(debug.message_id, id);
    assert_eq!(debug.thread_id, "thread-abc");

    // Human diagnostics remain available for restricted mail.
    store
        .correct_triage(acct, id, TriageAxis::Sensitivity, "sealed", None, t0)
        .unwrap()
        .unwrap();
    assert!(store.triage_debug(acct, id).unwrap().is_some());
}

#[test]
fn correcting_an_unknown_message_is_none_not_an_error() {
    let (store, acct) = store();
    let t0 = Utc::now();
    assert!(
        store
            .correct_triage(acct, 9999, TriageAxis::Category, "invoice", None, t0)
            .unwrap()
            .is_none()
    );
}
