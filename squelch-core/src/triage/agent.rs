//! Bounded structured-output investigation. The model owns semantics; this
//! executor only limits resources and validates references and output shape.
use super::{
    agent_config::AgentTriageConfig,
    context::*,
    decision::*,
    llm::{self, LlmOutcome, LlmRequest, Usage},
};
use crate::config::Stage2Provider;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const PROMPT_VERSION: &str = "agent-triage-v1";
const SYSTEM: &str = include_str!("prompts/agent-v1.txt");
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum AgentStep {
    ReadEvidence { requests: Vec<EvidenceRequest> },
    RequestReview { question: String },
    Finish { decision: Box<MessageDecision> },
}
pub struct AgentRun {
    pub decision: MessageDecision,
    pub source_message_ids: Vec<i64>,
    pub thread_revisions: BTreeMap<String, i64>,
    pub usage: Vec<Usage>,
    pub model_turns: usize,
    pub tool_calls: usize,
}
pub struct AgentConnection<'a> {
    pub http: &'a reqwest::Client,
    pub url: &'a str,
    pub api_key: &'a str,
    pub provider: Stage2Provider,
    pub model: &'a str,
    pub effort: Option<&'a str>,
}
/// Failed investigations still report usage for completed provider responses.
/// A timed-out in-flight request may be billed without returning provider usage.
#[derive(Debug)]
pub struct AgentFailure {
    pub kind: String,
    pub usage: Vec<Usage>,
    /// Number of attempted turns; a rejected first call may be refunded safely.
    pub model_calls: usize,
}
impl std::fmt::Display for AgentFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.kind)
    }
}
impl std::error::Error for AgentFailure {}

pub async fn run_agent(
    connection: AgentConnection<'_>,
    config: &AgentTriageConfig,
    mut context: ContextSnapshot,
    reader: &dyn EvidenceReader,
) -> Result<AgentRun, AgentFailure> {
    config.validate().map_err(|kind| AgentFailure {
        kind,
        usage: Vec::new(),
        model_calls: 0,
    })?;
    // Reserve half the context window for investigation results. Truncation is
    // visible in the payload and never silently drops a message's identity.
    bound_evidence_text(&mut context.initial, config.context.max_context_bytes / 2);
    let mut usage = Vec::new();
    let mut model_calls = 0;
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(config.agent.timeout_secs),
        run_bounded(
            connection,
            config,
            context,
            reader,
            &mut usage,
            &mut model_calls,
        ),
    )
    .await;
    match outcome {
        Ok(Ok(run)) => Ok(run),
        Ok(Err(kind)) => Err(AgentFailure {
            kind,
            usage,
            model_calls,
        }),
        Err(_) => Err(AgentFailure {
            kind: "agent_timeout".into(),
            usage,
            model_calls,
        }),
    }
}

async fn run_bounded(
    connection: AgentConnection<'_>,
    config: &AgentTriageConfig,
    context: ContextSnapshot,
    reader: &dyn EvidenceReader,
    usage: &mut Vec<Usage>,
    model_calls: &mut usize,
) -> Result<AgentRun, String> {
    let mut sources: BTreeSet<i64> = context.source_message_ids.iter().copied().collect();
    let mut revisions = context.thread_revisions.clone();
    let mut observed = ObservedDecisionContext::default();
    observed.observe(&context.initial, Some(&context.thread_id));
    let mut history = Vec::<Value>::new();
    let mut tool_calls = 0;
    let mut reviews = 0;
    let mut model = config.agent.model.as_deref().unwrap_or(connection.model);
    for turn in 0..config.agent.max_model_turns {
        let final_turn = turn + 1 == config.agent.max_model_turns;
        let user = serde_json::to_string(
            &json!({"context": context, "investigation": history, "must_finish": final_turn}),
        )
        .map_err(|_| "context_encode")?;
        if user.len() > config.context.max_context_bytes {
            return Err("context_budget_exceeded".into());
        }
        let request = LlmRequest {
            model,
            system: SYSTEM,
            user: &user,
            schema: step_schema(),
            effort: connection.effort,
            max_tries: 1,
        };
        // Retain provider usage before decoding either JSON syntax or step
        // structure. A malformed paid answer still belongs in the spend ledger.
        *model_calls += 1;
        let response = llm::classify_llm(
            connection.http,
            connection.url,
            connection.api_key,
            connection.provider,
            &request,
        )
        .await
        .map_err(|error| error.kind)?;
        // A review request grants one stronger-model call. Follow-up tool
        // investigation returns to the base model unless another review is
        // explicitly requested within the remaining review budget.
        model = config.agent.model.as_deref().unwrap_or(connection.model);
        usage.extend(response.usage());
        let step = match response {
            LlmOutcome::Ok(value, _) => match serde_json::from_str::<WireStep>(&value) {
                Ok(wire) => wire.result,
                Err(_) => {
                    history.push(json!({"validation_error": "invalid_step_shape"}));
                    continue;
                }
            },
            LlmOutcome::Refused(_) => return Err("agent_refused".into()),
            LlmOutcome::Failed(kind, _) => {
                if llm::is_config_failure(&kind) {
                    return Err(kind);
                }
                history.push(json!({"validation_error": kind,
                    "instruction": "Return a valid structured step."}));
                continue;
            }
        };
        match step {
            AgentStep::Finish { decision } => {
                match validate_decision(&decision, &sources, &revisions, &context.rule_ids)
                    .and_then(|()| observed.validate(&decision, &context.thread_id))
                {
                    Ok(()) => {
                        return Ok(AgentRun {
                            decision: *decision,
                            source_message_ids: sources.into_iter().collect(),
                            thread_revisions: revisions,
                            usage: usage.clone(),
                            model_turns: turn + 1,
                            tool_calls,
                        });
                    }
                    Err(error) => history.push(json!({"validation_error": error,
                        "instruction": "Repair your final decision using observed evidence."})),
                }
            }
            AgentStep::ReadEvidence { requests } => {
                if final_turn
                    || requests.is_empty()
                    || tool_calls + requests.len() > config.agent.max_tool_calls
                {
                    return Err("agent_tool_budget_exceeded".into());
                }
                for request in requests {
                    tool_calls += 1;
                    match reader.read(&request) {
                        Ok(mut result) => {
                            bound_evidence_text(
                                &mut result.data,
                                config.context.max_tool_result_bytes,
                            );
                            let encoded =
                                serde_json::to_vec(&result.data).map_err(|_| "tool_encode")?;
                            if encoded.len() > config.context.max_tool_result_bytes {
                                history.push(json!({"request": request,
                                    "error": "result_too_large; narrow the request"}));
                                continue;
                            }
                            observed.observe(&result.data, None);
                            sources.extend(result.source_message_ids.iter().copied());
                            for (thread, revision) in &result.thread_revisions {
                                if revisions
                                    .get(thread)
                                    .is_some_and(|existing| existing != revision)
                                {
                                    return Err("context_changed_during_run".into());
                                }
                            }
                            revisions.extend(result.thread_revisions.clone());
                            history.push(json!({"request": request, "result": result.data}));
                        }
                        Err(_) => history.push(json!({"request": request,
                            "error": "evidence_unavailable"})),
                    }
                }
            }
            AgentStep::RequestReview { question } => {
                if final_turn || reviews >= config.agent.max_review_calls {
                    return Err("agent_review_budget_exceeded".into());
                }
                if let Some(review_model) = config.agent.review_model.as_deref() {
                    model = review_model;
                    reviews += 1;
                    history.push(json!({"review_question": question}));
                } else {
                    history.push(json!({"error":
                        "review_model_unavailable; investigate with available tools or finish"}));
                }
            }
        }
    }
    Err("agent_turn_budget_exhausted".into())
}

/// Bound large textual evidence without truncating JSON, identifiers, user
/// preferences, or timestamps. Each shortened field gets a visible indicator.
fn bound_evidence_text(value: &mut Value, budget: usize) {
    if serde_json::to_vec(value).is_ok_and(|encoded| encoded.len() <= budget) {
        return;
    }
    fn count(value: &Value) -> usize {
        match value {
            Value::Object(fields) => fields
                .iter()
                .map(|(key, value)| {
                    if matches!(key.as_str(), "body" | "content" | "text" | "snippet")
                        && value.is_string()
                    {
                        1
                    } else {
                        count(value)
                    }
                })
                .sum(),
            Value::Array(values) => values.iter().map(count).sum(),
            _ => 0,
        }
    }
    fn trim(value: &mut Value, limit: usize) {
        match value {
            Value::Object(fields) => {
                let mut indicators = Vec::new();
                for (key, value) in fields.iter_mut() {
                    if matches!(key.as_str(), "body" | "content" | "text" | "snippet")
                        && let Value::String(text) = value
                    {
                        if text.len() > limit {
                            let mut end = limit;
                            while !text.is_char_boundary(end) {
                                end -= 1;
                            }
                            text.truncate(end);
                            indicators.push(format!("{key}_truncated"));
                        }
                        continue;
                    }
                    trim(value, limit);
                }
                for key in indicators {
                    // If a paged body is shortened again by the overall
                    // context bound, resume at the visible boundary, not the
                    // original page end (which would skip unseen evidence).
                    if key == "body_truncated"
                        && let (Some(offset), Some(body)) = (
                            fields.get("body_offset").and_then(Value::as_u64),
                            fields.get("body").and_then(Value::as_str),
                        )
                    {
                        let next = offset.saturating_add(body.chars().count() as u64);
                        fields.insert("next_offset".into(), json!(next));
                    }
                    fields.insert(key, Value::Bool(true));
                }
            }
            Value::Array(values) => {
                for value in values {
                    trim(value, limit);
                }
            }
            _ => {}
        }
    }
    let fields = count(value).max(1);
    // JSON escaping can expand a byte up to six times. Leave structural room.
    trim(value, budget / (fields * 8));
}

/// Structural validation only: no category clamps, auth detector, or score floor.
/// Structural facts captured from the evidence actually shown to the model.
/// Invalid action IDs, cross-thread relevance, and self-targeted updates get a
/// repair turn before commit instead of buying an entirely new investigation.
#[derive(Default)]
struct ObservedDecisionContext {
    message_threads: BTreeMap<i64, String>,
    action_ids: BTreeMap<String, BTreeSet<String>>,
}
impl ObservedDecisionContext {
    fn observe(&mut self, data: &Value, current_thread: Option<&str>) {
        let mut thread = current_thread.map(str::to_owned);
        let messages = data.get("message").into_iter().chain(
            ["messages", "thread"]
                .into_iter()
                .filter_map(|field| data.get(field).and_then(Value::as_array))
                .flatten(),
        );
        for message in messages {
            if let (Some(id), Some(name)) = (
                message.get("id").and_then(Value::as_i64),
                message.get("thread_id").and_then(Value::as_str),
            ) {
                self.message_threads.insert(id, name.into());
                thread.get_or_insert_with(|| name.into());
            }
        }
        if let Some(thread) = thread
            && let Some(attention) = data.get("attention")
        {
            self.action_ids.insert(
                thread,
                attention
                    .get("actions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|action| {
                        action.get("id").and_then(Value::as_str).map(str::to_owned)
                    })
                    .collect(),
            );
        }
    }
    fn validate(&self, decision: &MessageDecision, thread: &str) -> Result<(), String> {
        let check = |attention: &ThreadAttentionDecision, target: &str| -> Result<(), String> {
            if attention
                .relevant_message_ids
                .iter()
                .any(|id| self.message_threads.get(id).map(String::as_str) != Some(target))
            {
                return Err("attention_relevance_must_belong_to_target_thread".into());
            }
            let mut seen = BTreeSet::new();
            for id in attention.actions.iter().filter_map(|a| a.id.as_ref()) {
                if !seen.insert(id)
                    || !self
                        .action_ids
                        .get(target)
                        .is_some_and(|ids| ids.contains(id))
                {
                    return Err("unknown_or_duplicate_attention_action_id".into());
                }
            }
            Ok(())
        };
        check(&decision.attention, thread)?;
        let mut targets = BTreeSet::new();
        for update in &decision.related_updates {
            if update.thread_id == thread || !targets.insert(&update.thread_id) {
                return Err("related_update_repeats_thread".into());
            }
            if !self.action_ids.contains_key(&update.thread_id) {
                return Err("related_attention_must_be_read".into());
            }
            check(&update.attention, &update.thread_id)?;
        }
        Ok(())
    }
}

pub fn validate_decision(
    decision: &MessageDecision,
    sources: &BTreeSet<i64>,
    revisions: &BTreeMap<String, i64>,
    rule_ids: &[i64],
) -> Result<(), String> {
    let fail = |kind: &str| Err(kind.to_owned());
    if decision.kinds.is_empty()
        || decision.summary.trim().is_empty()
        || decision.reason.trim().is_empty()
    {
        return fail("decision_missing_explanation");
    }
    if decision
        .kinds
        .iter()
        .enumerate()
        .any(|(i, k)| decision.kinds[..i].contains(k))
        || decision
            .destinations
            .iter()
            .enumerate()
            .any(|(i, d)| decision.destinations[..i].contains(d))
    {
        return fail("duplicate_classification");
    }
    if decision.notification.importance > 100 {
        return fail("notification_score_out_of_range");
    }
    let evidence = |items: &[EvidenceRef]| {
        !items.is_empty()
            && items.iter().all(|e| {
                sources.contains(&e.message_id)
                    && !e.location.trim().is_empty()
                    && e.location.len() <= 256
            })
    };
    if decision.external_access.reason.trim().is_empty()
        || !evidence(&decision.external_access.evidence)
    {
        return fail("access_requires_evidence");
    }
    if decision.auth.is_auth() && !evidence(&decision.auth.evidence) {
        return fail("auth_requires_evidence");
    }
    let supported_time = |time: &SupportedTime| {
        sources.contains(&time.source_message_id)
            && (chrono::DateTime::parse_from_rfc3339(&time.value).is_ok()
                || chrono::NaiveDate::parse_from_str(&time.value, "%Y-%m-%d").is_ok())
    };
    let attention = |a: &ThreadAttentionDecision| {
        [
            a.factors.urgency,
            a.factors.action_need,
            a.factors.personal_relevance,
            a.factors.importance,
        ]
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && a.factors.attention_at.as_ref().is_none_or(&supported_time)
            && !a.relevant_message_ids.is_empty()
            && a.relevant_message_ids.iter().all(|id| sources.contains(id))
            && evidence(&a.evidence)
            && a.actions.iter().all(|action| {
                !action.description.trim().is_empty()
                    && evidence(&action.evidence)
                    && action.due.as_ref().is_none_or(&supported_time)
            })
    };
    if !attention(&decision.attention) {
        return fail("invalid_attention_evidence_or_factors");
    }
    for update in &decision.related_updates {
        if revisions.get(&update.thread_id) != Some(&update.expected_revision)
            || !evidence(&update.evidence)
            || !attention(&update.attention)
        {
            return fail("related_update_not_observed");
        }
    }
    for exception in &decision.rule_exceptions {
        if !rule_ids.contains(&exception.rule_id)
            || exception.reason.trim().is_empty()
            || !evidence(&exception.evidence)
        {
            return fail("invalid_rule_exception");
        }
    }
    for record in &decision.records {
        if !evidence(record.evidence()) {
            return fail("record_requires_evidence");
        }
    }
    if decision
        .revisit
        .as_ref()
        .is_some_and(|r| r.reason.trim().is_empty())
    {
        return fail("revisit_requires_reason");
    }
    if decision
        .revisit
        .as_ref()
        .is_some_and(|r| r.at <= chrono::Utc::now())
    {
        return fail("revisit_must_be_in_the_future");
    }
    Ok(())
}

fn object(fields: Vec<(&str, Value)>) -> Value {
    let required: Vec<&str> = fields.iter().map(|(key, _)| *key).collect();
    let properties: serde_json::Map<String, Value> = fields
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect();
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn array(value: Value) -> Value {
    json!({"type":"array","items":value})
}
fn nullable(value: Value) -> Value {
    json!({"anyOf":[value,{"type":"null"}]})
}
fn enumeration(values: &[&str]) -> Value {
    json!({"type":"string","enum":values})
}
fn text() -> Value {
    json!({"type":"string"})
}
fn integer() -> Value {
    json!({"type":"integer"})
}
fn number() -> Value {
    json!({"type":"number"})
}
fn boolean() -> Value {
    json!({"type":"boolean"})
}
fn evidence_schema() -> Value {
    array(object(vec![
        ("message_id", integer()),
        ("location", text()),
    ]))
}
fn time_schema() -> Value {
    object(vec![
        ("value", text()),
        ("timezone", nullable(text())),
        ("source_message_id", integer()),
        ("interpreted_relative", boolean()),
    ])
}
fn attention_schema() -> Value {
    object(vec![
        ("show_in_fye", boolean()),
        (
            "state",
            enumeration(&[
                "informational",
                "needs_user",
                "waiting_on_others",
                "resolved",
            ]),
        ),
        (
            "actions",
            array(object(vec![
                ("id", nullable(text())),
                (
                    "kind",
                    enumeration(&["review", "reply", "pay", "decide", "attend", "other"]),
                ),
                ("description", text()),
                ("resolved", boolean()),
                ("due", nullable(time_schema())),
                ("evidence", evidence_schema()),
            ])),
        ),
        ("summary", text()),
        (
            "factors",
            object(vec![
                ("urgency", number()),
                ("action_need", number()),
                ("personal_relevance", number()),
                ("importance", number()),
                ("attention_at", nullable(time_schema())),
            ]),
        ),
        ("relevant_message_ids", array(integer())),
        ("evidence", evidence_schema()),
    ])
}
pub fn decision_schema() -> Value {
    let records = vec![
        object(vec![
            ("kind", enumeration(&["bill"])),
            ("merchant", text()),
            ("amount", nullable(number())),
            ("currency", nullable(text())),
            ("due", nullable(time_schema())),
            ("autopay", nullable(boolean())),
            ("evidence", evidence_schema()),
        ]),
        object(vec![
            ("kind", enumeration(&["receipt"])),
            ("merchant", text()),
            ("amount", nullable(number())),
            ("currency", nullable(text())),
            ("evidence", evidence_schema()),
        ]),
        object(vec![
            ("kind", enumeration(&["delivery"])),
            (
                "carrier",
                nullable(enumeration(&[
                    "ups", "usps", "fedex", "dhl", "amazon", "unknown",
                ])),
            ),
            ("tracking_number", nullable(text())),
            (
                "status",
                enumeration(&[
                    "ordered",
                    "shipped",
                    "out_for_delivery",
                    "delivered",
                    "exception",
                    "unknown",
                ]),
            ),
            ("evidence", evidence_schema()),
        ]),
        object(vec![
            ("kind", enumeration(&["event"])),
            ("title", text()),
            ("start", nullable(time_schema())),
            ("location", nullable(text())),
            ("evidence", evidence_schema()),
        ]),
        object(vec![
            ("kind", enumeration(&["financial_update"])),
            ("institution", text()),
            ("description", text()),
            ("evidence", evidence_schema()),
        ]),
    ];
    object(vec![
        (
            "kinds",
            array(enumeration(&[
                "general",
                "correspondence",
                "editorial",
                "promotional",
                "bill",
                "receipt",
                "financial_update",
                "delivery",
                "event_reservation",
                "account_service",
                "authentication_security",
            ])),
        ),
        ("destinations", array(enumeration(&["reading", "records"]))),
        ("summary", text()),
        ("reason", text()),
        (
            "auth",
            object(vec![
                (
                    "kinds",
                    array(enumeration(&[
                        "otp",
                        "password_reset",
                        "sign_in_link",
                        "verification",
                        "login_alert",
                        "security_alert",
                    ])),
                ),
                ("evidence", evidence_schema()),
            ]),
        ),
        (
            "external_access",
            object(vec![
                ("restricted", boolean()),
                ("reason", text()),
                ("evidence", evidence_schema()),
            ]),
        ),
        ("attention", attention_schema()),
        ("records", array(json!({"anyOf":records}))),
        (
            "related_updates",
            array(object(vec![
                ("thread_id", text()),
                ("expected_revision", integer()),
                ("attention", attention_schema()),
                ("evidence", evidence_schema()),
            ])),
        ),
        (
            "rule_exceptions",
            array(object(vec![
                ("rule_id", integer()),
                ("reason", text()),
                ("evidence", evidence_schema()),
            ])),
        ),
        (
            "notification",
            object(vec![
                ("importance", integer()),
                ("title", text()),
                ("body", text()),
                ("reason", text()),
            ]),
        ),
        (
            "revisit",
            nullable(object(vec![("at", text()), ("reason", text())])),
        ),
    ])
}
fn step_schema() -> Value {
    let requests = vec![
        object(vec![
            ("tool", enumeration(&["read_thread"])),
            ("thread_id", text()),
        ]),
        object(vec![
            ("tool", enumeration(&["read_message"])),
            ("message_id", integer()),
            ("offset", nullable(integer())),
        ]),
        object(vec![
            ("tool", enumeration(&["search_mail"])),
            ("query", text()),
        ]),
        object(vec![
            ("tool", enumeration(&["read_sender_history"])),
            ("sender", text()),
        ]),
        object(vec![
            ("tool", enumeration(&["read_record"])),
            ("message_id", integer()),
        ]),
        object(vec![
            ("tool", enumeration(&["read_attachment"])),
            ("message_id", integer()),
            ("attachment_id", text()),
        ]),
    ];
    let steps = vec![
        object(vec![
            ("step", enumeration(&["read_evidence"])),
            ("requests", array(json!({"anyOf": requests}))),
        ]),
        object(vec![
            ("step", enumeration(&["request_review"])),
            ("question", text()),
        ]),
        object(vec![
            ("step", enumeration(&["finish"])),
            ("decision", decision_schema()),
        ]),
    ];
    // Provider structured outputs require an object at the root; a nested
    // union is supported by both transports. WireStep unwraps that envelope.
    object(vec![("result", json!({"anyOf": steps}))])
}
#[derive(Deserialize)]
struct WireStep {
    result: AgentStep,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> MessageDecision {
        let evidence = vec![EvidenceRef {
            message_id: 7,
            location: "body".into(),
        }];
        MessageDecision {
            kinds: vec![EmailKind::General],
            summary: "A message".into(),
            reason: "Informational".into(),
            external_access: AccessAssessment {
                restricted: false,
                reason: "No credentials".into(),
                evidence: evidence.clone(),
            },
            attention: ThreadAttentionDecision {
                relevant_message_ids: vec![7],
                evidence,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[test]
    fn credentials_are_not_inferred_from_auth_category() {
        let mut decision = valid();
        decision.auth = AuthAssessment {
            kinds: vec![AuthKind::LoginAlert],
            evidence: decision.attention.evidence.clone(),
        };
        assert!(validate_decision(&decision, &BTreeSet::from([7]), &BTreeMap::new(), &[]).is_ok());
        assert!(!decision.external_access.restricted);
    }
    #[test]
    fn past_revisits_cannot_create_an_immediate_investigation_loop() {
        let mut decision = valid();
        decision.revisit = Some(RevisitRequest {
            at: chrono::Utc::now() - chrono::Duration::minutes(1),
            reason: "Check later".into(),
        });
        assert_eq!(
            validate_decision(&decision, &BTreeSet::from([7]), &BTreeMap::new(), &[]),
            Err("revisit_must_be_in_the_future".into())
        );
        decision.revisit.as_mut().unwrap().at = chrono::Utc::now() + chrono::Duration::hours(1);
        assert!(validate_decision(&decision, &BTreeSet::from([7]), &BTreeMap::new(), &[]).is_ok());
    }
    #[test]
    fn unknown_evidence_and_unfetched_updates_fail() {
        let mut decision = valid();
        decision.attention.relevant_message_ids = vec![8];
        assert!(validate_decision(&decision, &BTreeSet::from([7]), &BTreeMap::new(), &[]).is_err());
        decision = valid();
        decision.related_updates.push(RelatedAttentionUpdate {
            thread_id: "other".into(),
            expected_revision: 1,
            attention: decision.attention.clone(),
            evidence: decision.attention.evidence.clone(),
        });
        assert!(validate_decision(&decision, &BTreeSet::from([7]), &BTreeMap::new(), &[]).is_err());
    }
    #[test]
    fn delivery_schema_statuses_are_compatible_with_carrier_projection() {
        let schema = decision_schema();
        let delivery = schema["properties"]["records"]["items"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["properties"]["kind"]["enum"] == json!(["delivery"]))
            .unwrap();
        let statuses = delivery["properties"]["status"]["enum"].as_array().unwrap();
        assert_eq!(statuses.len(), 6);
        for status in statuses {
            let value = status.as_str().unwrap();
            assert!(
                value == "unknown" || crate::triage::ShipmentStatus::parse(value).is_some(),
                "schema status must be pollable or explicitly unknown: {value}"
            );
        }
    }
    #[test]
    fn strict_schema_requires_all_object_fields() {
        fn check(value: &Value) {
            if value.get("type") == Some(&json!("object")) {
                assert_eq!(value["additionalProperties"], json!(false));
                assert_eq!(
                    value["required"].as_array().unwrap().len(),
                    value["properties"].as_object().unwrap().len()
                );
            }
            match value {
                Value::Object(map) => {
                    for v in map.values() {
                        check(v)
                    }
                }
                Value::Array(items) => {
                    for v in items {
                        check(v)
                    }
                }
                _ => {}
            }
        }
        check(&step_schema());
    }
    struct NoEvidence;
    impl EvidenceReader for NoEvidence {
        fn read(&self, _: &EvidenceRequest) -> Result<EvidenceResult, String> {
            Err("unavailable".into())
        }
    }

    async fn mock_provider(
        axum::extract::State(responses): axum::extract::State<
            std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<Value>>>,
        >,
        axum::Json(request): axum::Json<Value>,
    ) -> axum::Json<Value> {
        assert_eq!(request["response_format"]["json_schema"]["strict"], true);
        let content = responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("bounded calls");
        axum::Json(json!({
            "choices": [{"message": {"content": content.as_str().map(str::to_owned).unwrap_or_else(|| content.to_string())}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 8}
        }))
    }

    #[tokio::test]
    async fn provider_step_repair_retains_usage_and_validates_evidence() {
        use axum::{Router, routing::post};
        use std::sync::{Arc, Mutex};
        let mut invalid = valid();
        invalid.attention.relevant_message_ids = vec![999];
        let responses = Arc::new(Mutex::new(std::collections::VecDeque::from([
            json!("{malformed-json"),
            json!({"result": {"step": "finish", "decision": invalid}}),
            json!({"result": {"step": "finish", "decision": valid()}}),
        ])));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/", post(mock_provider))
            .with_state(responses);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let http = reqwest::Client::new();
        let connection = AgentConnection {
            http: &http,
            url: &url,
            api_key: "test",
            provider: Stage2Provider::OpenAI,
            model: "test",
            effort: None,
        };
        let context = ContextSnapshot {
            thread_id: "thread".into(),
            initial: json!({"message":{"id":7,"thread_id":"thread"},"attention":null}),
            source_message_ids: vec![7],
            ..Default::default()
        };
        let run = run_agent(
            connection,
            &AgentTriageConfig::default(),
            context,
            &NoEvidence,
        )
        .await
        .unwrap();
        assert_eq!(run.model_turns, 3);
        assert_eq!(run.usage.len(), 3);
        assert_eq!(run.usage.iter().map(|u| u.input_tokens).sum::<u64>(), 36);
        server.abort();
    }

    #[tokio::test]
    async fn commit_constraints_are_repaired_inside_the_paid_investigation() {
        use axum::{Router, routing::post};
        use std::sync::{Arc, Mutex};
        for invalid_kind in 0..3 {
            let mut invalid = valid();
            match invalid_kind {
                0 => invalid.attention.relevant_message_ids = vec![8],
                1 => invalid.attention.actions.push(AttentionAction {
                    id: Some("invented".into()),
                    description: "Pay".into(),
                    evidence: invalid.attention.evidence.clone(),
                    ..Default::default()
                }),
                _ => invalid.related_updates.push(RelatedAttentionUpdate {
                    thread_id: "thread".into(),
                    expected_revision: 1,
                    attention: invalid.attention.clone(),
                    evidence: invalid.attention.evidence.clone(),
                }),
            }
            let responses = Arc::new(Mutex::new(std::collections::VecDeque::from([
                json!({"result":{"step":"finish","decision":invalid}}),
                json!({"result":{"step":"finish","decision":valid()}}),
            ])));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let app = Router::new()
                .route("/", post(mock_provider))
                .with_state(responses);
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let http = reqwest::Client::new();
            let connection = AgentConnection {
                http: &http,
                url: &url,
                api_key: "test",
                provider: Stage2Provider::OpenAI,
                model: "test",
                effort: None,
            };
            let context = ContextSnapshot {
                thread_id: "thread".into(),
                source_message_ids: vec![7, 8],
                thread_revisions: BTreeMap::from([("thread".into(), 1)]),
                initial: json!({"message":{"id":7,"thread_id":"thread"},"thread":[{"id":8,"thread_id":"other"}],"attention":null}),
                ..Default::default()
            };
            let run = run_agent(
                connection,
                &AgentTriageConfig::default(),
                context,
                &NoEvidence,
            )
            .await
            .unwrap();
            assert_eq!(run.model_turns, 2, "{invalid_kind}");
            assert_eq!(run.usage.len(), 2);
            server.abort();
        }
    }

    #[test]
    fn text_limits_are_visible_and_preserve_utf8_and_identity() {
        let mut value = json!({"message_id": 7, "body": "é".repeat(10000)});
        bound_evidence_text(&mut value, 1000);
        assert_eq!(value["message_id"], 7);
        assert_eq!(value["body_truncated"], true);
        assert!(value["body"].as_str().unwrap().len() <= 125);
    }
    struct ExtraEvidence;
    impl EvidenceReader for ExtraEvidence {
        fn read(&self, request: &EvidenceRequest) -> Result<EvidenceResult, String> {
            assert!(matches!(request, EvidenceRequest::SearchMail { .. }));
            Ok(EvidenceResult {
                data: json!({"messages": [{"id": 8, "snippet": "Additional evidence"}]}),
                source_message_ids: vec![8],
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn uncited_tool_sources_remain_in_derivative_provenance() {
        use axum::{Router, routing::post};
        use std::sync::{Arc, Mutex};
        let responses = Arc::new(Mutex::new(std::collections::VecDeque::from([
            json!({"result": {"step": "read_evidence", "requests": [{"tool": "search_mail", "query": "invoice"}]}}),
            json!({"result": {"step": "finish", "decision": valid()}}),
        ])));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/", post(mock_provider))
            .with_state(responses);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let http = reqwest::Client::new();
        let connection = AgentConnection {
            http: &http,
            url: &url,
            api_key: "test",
            provider: Stage2Provider::OpenAI,
            model: "test",
            effort: None,
        };
        let context = ContextSnapshot {
            thread_id: "thread".into(),
            initial: json!({"message":{"id":7,"thread_id":"thread"},"attention":null}),
            source_message_ids: vec![7],
            ..Default::default()
        };
        let run = run_agent(
            connection,
            &AgentTriageConfig::default(),
            context,
            &ExtraEvidence,
        )
        .await
        .unwrap();
        assert_eq!(run.source_message_ids, vec![7, 8]);
        assert_eq!(run.tool_calls, 1);
        assert_eq!(run.decision.attention.evidence[0].message_id, 7);
        server.abort();
    }

    #[tokio::test]
    async fn failed_validation_still_reports_billed_usage() {
        use axum::{Router, routing::post};
        use std::sync::{Arc, Mutex};
        let responses = Arc::new(Mutex::new(std::collections::VecDeque::from([
            json!({"result": {"step": "finish", "decision": MessageDecision::default()}}),
        ])));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/", post(mock_provider))
            .with_state(responses);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let http = reqwest::Client::new();
        let connection = AgentConnection {
            http: &http,
            url: &url,
            api_key: "test",
            provider: Stage2Provider::OpenAI,
            model: "test",
            effort: None,
        };
        let mut config = AgentTriageConfig::default();
        config.agent.max_model_turns = 1;
        let failure = run_agent(connection, &config, ContextSnapshot::default(), &NoEvidence)
            .await
            .err()
            .unwrap();
        assert_eq!(failure.kind, "agent_turn_budget_exhausted");
        assert_eq!(failure.usage.len(), 1);
        assert_eq!(failure.usage[0].input_tokens, 12);
        server.abort();
    }
    #[test]
    fn shortened_pages_resume_at_visible_unicode_boundary() {
        let mut value = json!({"body": "é".repeat(10000), "body_offset": 50, "next_offset": 10050});
        bound_evidence_text(&mut value, 1000);
        let visible = value["body"].as_str().unwrap().chars().count();
        assert_eq!(value["next_offset"], json!(50 + visible));
    }
}
