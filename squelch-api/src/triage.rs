//! Versioned human projections for agent-owned triage.
use crate::{ApiError, ApiState, handlers::store_call};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::header,
    response::IntoResponse,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use squelch_core::store::{Store, agent_triage::AgentTriageStore};

#[derive(Deserialize)]
pub struct FeedQuery {
    pub destination: String,
    pub limit: Option<usize>,
}

pub async fn feed(
    State(state): State<ApiState>,
    Query(query): Query<FeedQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let ranking = state.triage_config.ranking.clone();
    let now = Utc::now();
    let mut items = store_call(&state, move |store, account| {
        match query.destination.as_str() {
            "fye" => store.agent_fye(account, usize::MAX, &ranking, now),
            "reading" => store.agent_reading(account, usize::MAX),
            "records" => store.agent_records(account, usize::MAX),
            _ => Err(squelch_core::CoreError::InvalidInput(
                "unsupported destination".into(),
            )),
        }
    })
    .await?;
    let total_count = items.len();
    items.truncate(limit);
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"version": 2, "ranked_at": now, "total_count": total_count, "items": items})),
    ))
}

pub async fn message(
    State(state): State<ApiState>,
    Path(message_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let view = store_call(&state, move |store, account| {
        let message = store.agent_read_message(account, message_id)?;
        let mut thread =
            serde_json::to_value(store.thread_view_with_html(account, &message.thread_id)?)
                .map_err(|error| squelch_core::CoreError::Other(error.into()))?;
        thread["cache_allowed"] = store
            .external_thread_allowed(account, &message.thread_id)?
            .into();
        Ok(json!({"message_id": message_id, "thread": thread}))
    })
    .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view)))
}

pub async fn opened(
    State(state): State<ApiState>,
    Path(message_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    store_call(&state, move |store, account| {
        store.acknowledge_agent_message(account, message_id, Utc::now())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn capabilities() -> impl IntoResponse {
    Json(
        json!({"triage_version": 2, "server_ranked_fye": true, "reading": true, "pending_message_read": true}),
    )
}

#[derive(Deserialize)]
pub struct Correction {
    pub field: String,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub add: Option<Vec<String>>,
    #[serde(default)]
    pub remove: Option<Vec<String>>,
}

pub async fn correct(
    State(state): State<ApiState>,
    Path(message_id): Path<i64>,
    Json(correction): Json<Correction>,
) -> Result<impl IntoResponse, ApiError> {
    store_call(&state, move |store, account| {
        if correction.add.is_some() || correction.remove.is_some() {
            if correction.value.is_some() {
                return Err(squelch_core::error::CoreError::InvalidInput(
                    "choose either value or add/remove".into(),
                ));
            }
            store.correct_agent_triage_delta(
                account,
                message_id,
                &correction.field,
                correction.add.as_deref().unwrap_or(&[]),
                correction.remove.as_deref().unwrap_or(&[]),
                Utc::now(),
            )
        } else {
            let value = correction.value.ok_or_else(|| {
                squelch_core::error::CoreError::InvalidInput(
                    "correction requires value or add/remove".into(),
                )
            })?;
            store.correct_agent_triage(account, message_id, &correction.field, &value, Utc::now())
        }
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn decision(
    State(state): State<ApiState>,
    Path(message_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let view = store_call(&state, move |store, account| {
        let context = store.load_agent_context(&squelch_core::store::agent_triage::AgentJob {
            id: 0, account_id: account, message_id, kind: "triage".into(), trigger: "inspect".into(),
            lease_token: String::new(), attempts: 0, arrival_eligible: false, foreground: false,
        })?;
        let diagnostics = store.agent_diagnostics(account, message_id)?;
        let fast_notification = store.latest_notification_assessment(account, message_id,
            squelch_core::metrics::NotifyLane::Fast)?;
        Ok(json!({"version": 2, "decision": context.previous_decision, "attention": context.attention,
            "corrections": context.corrections, "pending": context.previous_decision.is_none(),
            "diagnostics": diagnostics, "fast_notification": fast_notification}))
    }).await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view)))
}
