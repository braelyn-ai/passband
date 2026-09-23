//! The embedded assistant uses a distinct read audience from the human reader.
mod common;
use axum::http::StatusCode;
use chrono::Utc;
use common::*;
use squelch_core::{
    store::{
        Store,
        agent_triage::{AgentCommitOutcome, AgentTriageStore},
    },
    triage::decision::*,
};
use tower::ServiceExt;

fn assess(h: &Harness, id: i64, restricted: bool) {
    h.store
        .enqueue_agent_triage(h.acct, id, "test", false)
        .unwrap();
    let job = h
        .store
        .claim_agent_job(h.acct, "triage", Utc::now(), 60)
        .unwrap()
        .unwrap();
    assert_eq!(job.message_id, id);
    let context = h.store.load_agent_context(&job).unwrap();
    let evidence = vec![EvidenceRef {
        message_id: id,
        location: "body".into(),
    }];
    let decision = MessageDecision {
        kinds: vec![EmailKind::Receipt],
        destinations: vec![MessageDestination::Reading],
        summary: "A receipt".into(),
        reason: "Keep the record".into(),
        external_access: AccessAssessment {
            restricted,
            reason: "Access assessment".into(),
            evidence: evidence.clone(),
        },
        attention: ThreadAttentionDecision {
            show_in_fye: true,
            summary: "Review receipt".into(),
            relevant_message_ids: vec![id],
            evidence: evidence.clone(),
            ..Default::default()
        },
        records: vec![RecordProposal::Receipt {
            merchant: "Example".into(),
            amount: Some(12.0),
            currency: Some("USD".into()),
            evidence,
        }],
        notification: NotificationAdvice {
            importance: 30,
            title: "Internal notification title".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        h.store
            .commit_agent_decision(
                &job,
                &context,
                &decision,
                std::slice::from_ref(&context.message.source)
            )
            .unwrap(),
        AgentCommitOutcome::Applied
    );
}

#[tokio::test]
async fn human_can_open_pending_auth_but_agent_cannot_read_or_discover_it() {
    let h = harness(|_, _| {});
    let id = h
        .store
        .upsert_message(&msg(
            h.acct,
            "pending",
            "auth-thread",
            "secretcode",
            "Your code is 123456",
        ))
        .unwrap();
    let human = h
        .app
        .clone()
        .oneshot(authed("GET", &format!("/client/v2/messages/{id}")))
        .await
        .unwrap();
    assert_eq!(human.status(), StatusCode::OK);
    for url in [
        "/client/agent/thread/auth-thread".to_owned(),
        format!("/client/agent/triage/{id}"),
    ] {
        let response = h.app.clone().oneshot(authed("GET", &url)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{url}");
    }
    let search = h
        .app
        .clone()
        .oneshot(authed("GET", "/client/agent/search?q=secretcode"))
        .await
        .unwrap();
    assert_eq!(body_json(search).await["items"], serde_json::json!([]));
    assess(&h, id, true);
    let response = h
        .app
        .clone()
        .oneshot(authed("GET", "/client/agent/thread/auth-thread"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    for destination in ["fye", "reading", "records"] {
        let response = h
            .app
            .clone()
            .oneshot(authed(
                "GET",
                &format!("/client/agent/feed?destination={destination}"),
            ))
            .await
            .unwrap();
        assert_eq!(body_json(response).await["items"], serde_json::json!([]));
    }
    let response = h
        .app
        .oneshot(authed("GET", "/client/agent/records?kind=receipts"))
        .await
        .unwrap();
    assert_eq!(body_json(response).await["receipts"], serde_json::json!([]));
}

#[tokio::test]
async fn allowed_decisions_surface_without_internal_notification_metadata() {
    let h = harness(|_, _| {});
    let id = h
        .store
        .upsert_message(&msg(
            h.acct,
            "receipt",
            "receipt-thread",
            "receipt",
            "Paid twelve dollars",
        ))
        .unwrap();
    assess(&h, id, false);
    let response = h
        .app
        .clone()
        .oneshot(authed("GET", "/client/agent/feed"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = body_json(response).await;
    assert_eq!(body["items"][0]["message_id"], id);
    assert!(body["items"][0]["decision"].get("notification").is_none());
    let response = h
        .app
        .clone()
        .oneshot(authed("GET", &format!("/client/agent/triage/{id}")))
        .await
        .unwrap();
    let body = body_json(response).await;
    assert!(body["decision"].get("notification").is_none());
    let response = h
        .app
        .clone()
        .oneshot(authed("GET", "/client/agent/records?kind=receipts"))
        .await
        .unwrap();
    assert_eq!(
        body_json(response).await["receipts"][0]["record"]["amount"],
        12.0
    );
    // A new unassessed sibling invalidates the whole external thread context.
    h.store
        .upsert_message(&msg(h.acct, "new-auth", "receipt-thread", "Code", "123456"))
        .unwrap();
    let response = h
        .app
        .oneshot(authed("GET", "/client/agent/feed"))
        .await
        .unwrap();
    assert_eq!(body_json(response).await["items"], serde_json::json!([]));
}

#[tokio::test]
async fn agent_search_fills_results_after_pending_leading_matches() {
    let h = harness(|_, _| {});
    let mut readable = msg(h.acct, "readable", "readable-thread", "zebra", "zebra memo");
    readable.received_at = Utc::now() - chrono::Duration::days(10);
    let id = h.store.upsert_message(&readable).unwrap();
    assess(&h, id, false);
    for i in 0..24 {
        h.store
            .upsert_message(&msg(
                h.acct,
                &format!("pending-{i}"),
                &format!("pending-thread-{i}"),
                "zebra",
                "zebra memo",
            ))
            .unwrap();
    }
    let response = h
        .app
        .oneshot(authed(
            "GET",
            "/client/agent/search?q=zebra&limit=2&mode=keyword",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], id);
}
