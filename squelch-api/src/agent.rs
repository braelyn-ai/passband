//! Read endpoints for the embedded user agent. These never reuse the human
//! reader's unrestricted projections. Account identity comes from API state.
use crate::{ApiError, ApiState, handlers::store_call};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::header,
    response::IntoResponse,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use squelch_core::{
    CoreError,
    store::{
        SearchSort, Store,
        agent_triage::{AgentListItem, AgentTriageStore},
        parse_search_query,
    },
    triage::decision::{EmailKind, RecordProposal},
};

fn no_store(value: Value) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Json(value))
}

pub async fn thread(
    State(state): State<ApiState>,
    Path(thread_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let view = store_call(&state, move |store, account| {
        store.thread_view(account, &thread_id)
    })
    .await?;
    Ok(no_store(json!(view)))
}

#[derive(Deserialize)]
pub struct FeedQuery {
    #[serde(default = "default_destination")]
    destination: String,
    limit: Option<usize>,
}
fn default_destination() -> String {
    "fye".into()
}

/// Deliberately omit notification advice and executor diagnostics. The agent
/// gets the explanation and attention state needed to help its human.
fn feed_item(item: AgentListItem) -> Value {
    json!({"message_id": item.message_id, "thread_id": item.thread_id,
        "from_addr": item.from_addr, "subject": item.subject,
        "received_at": item.received_at, "score": item.score,
        "decision": {"kinds": item.decision.kinds, "destinations": item.decision.destinations,
            "summary": item.decision.summary, "reason": item.decision.reason},
        "attention": item.attention})
}

pub async fn feed(
    State(state): State<ApiState>,
    Query(query): Query<FeedQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let now = Utc::now();
    let ranking = state.triage_config.ranking.clone();
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let items = store_call(&state, move |store, account| {
        match query.destination.as_str() {
            "fye" => store.external_agent_fye(account, limit, &ranking, now),
            "reading" => store.external_agent_reading(account, limit),
            "records" => store.external_agent_records(account, limit),
            _ => Err(CoreError::InvalidInput("unsupported destination".into())),
        }
    })
    .await?;
    Ok(no_store(json!({"version": 2, "ranked_at": now,
        "items": items.into_iter().map(feed_item).collect::<Vec<_>>()})))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    limit: Option<u32>,
    mode: Option<String>,
}

pub async fn search(
    State(state): State<ApiState>,
    Query(query): Query<SearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (text, filter) = parse_search_query(query.q.trim());
    let text = text.trim().to_owned();
    if text.is_empty() && filter.is_empty() {
        return Err(ApiError::bad_request("q must not be empty"));
    }
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    let requested = query.mode.as_deref().unwrap_or("hybrid");
    if !matches!(requested, "keyword" | "semantic" | "hybrid") {
        return Err(ApiError::bad_request("unsupported search mode"));
    }
    let mode = if text.is_empty() || state.store.embedder().is_none() {
        "keyword"
    } else {
        requested
    }
    .to_owned();
    let response_mode = mode.clone();
    let hits = store_call(&state, move |store, account| {
        let mut window = limit as usize;
        loop {
            let (candidates, full) = match mode.as_str() {
                "hybrid" => store.hybrid_search(
                    account,
                    &text,
                    &filter,
                    SearchSort::default(),
                    false,
                    window,
                )?,
                "semantic" => store.semantic_search_hits(
                    account,
                    &text,
                    &filter,
                    SearchSort::default(),
                    false,
                    window.min(4096),
                )?,
                _ => {
                    let candidates = store.search_filtered(
                        account,
                        &text,
                        &filter,
                        SearchSort::default(),
                        false,
                        window.min(u32::MAX as usize) as u32,
                        0,
                    )?;
                    let full = candidates.len() == window;
                    (candidates, full)
                }
            };
            // Access is applied before the public result limit. A hidden leading
            // window must not hide a readable result farther down the ranking.
            let mut allowed = store.external_search_hits(account, &candidates)?;
            if allowed.len() >= limit as usize || !full || (mode == "semantic" && window >= 4096) {
                allowed.truncate(limit as usize);
                return Ok(allowed);
            }
            window = window.saturating_mul(2);
        }
    })
    .await?;
    Ok(no_store(
        json!({"items": hits, "match_kind": response_mode, "sort": "recent"}),
    ))
}

pub async fn decision(
    State(state): State<ApiState>,
    Path(message_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let decision = store_call(&state, move |store, account| {
        store.external_agent_decision(account, message_id)
    })
    .await?;
    Ok(no_store(
        json!({"decision": {"kinds": decision.kinds, "destinations": decision.destinations,
        "summary": decision.summary, "reason": decision.reason, "attention": decision.attention,
        "rule_exceptions": decision.rule_exceptions}}),
    ))
}

#[derive(Deserialize)]
pub struct RecordsQuery {
    kind: String,
    days: Option<u32>,
    hours: Option<u32>,
    #[serde(default)]
    include_delivered: bool,
}

fn record_matches(record: &RecordProposal, kind: &str, include_delivered: bool) -> bool {
    match (record, kind) {
        (RecordProposal::Bill { .. }, "bills")
        | (RecordProposal::Receipt { .. }, "receipts")
        | (RecordProposal::Event { .. }, "calendar")
        | (RecordProposal::FinancialUpdate { .. }, "banking") => true,
        (RecordProposal::Delivery { status, .. }, "shipments") => {
            include_delivered || status != "delivered"
        }
        _ => false,
    }
}

pub async fn records(
    State(state): State<ApiState>,
    Query(query): Query<RecordsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if !matches!(
        query.kind.as_str(),
        "bills" | "receipts" | "calendar" | "banking" | "shipments" | "marketing"
    ) {
        return Err(ApiError::bad_request("unsupported record kind"));
    }
    let marketing = query.kind == "marketing";
    let items = store_call(&state, move |store, account| {
        if marketing {
            store.external_agent_reading(account, 500)
        } else {
            store.external_agent_records(account, 500)
        }
    })
    .await?;
    let hours = query
        .hours
        .map(|hours| hours.clamp(1, 2160) as i64)
        .unwrap_or(query.days.unwrap_or(30).clamp(1, 90) as i64 * 24);
    let since = Utc::now() - Duration::hours(hours);
    let mut rows = Vec::new();
    for item in items {
        if DateTime::parse_from_rfc3339(&item.received_at).is_ok_and(|time| time < since) {
            continue;
        }
        if marketing {
            if item.decision.kinds.contains(&EmailKind::Promotional) {
                rows.push(
                    json!({"message_id": item.message_id, "thread_id": item.thread_id,
                    "received_at": item.received_at, "summary": item.decision.summary}),
                );
            }
            continue;
        }
        for record in &item.decision.records {
            if record_matches(record, &query.kind, query.include_delivered) {
                rows.push(
                    json!({"message_id": item.message_id, "thread_id": item.thread_id,
                    "received_at": item.received_at, "record": record}),
                );
            }
        }
    }
    rows.truncate(200);
    Ok(no_store(json!({(query.kind): rows})))
}
