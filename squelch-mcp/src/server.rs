//! Transport-agnostic MCP server: the [`SquelchServer`] handler and its tools.
//!
//! External reads require a current allowed assessment for every consumed source.
//! Pending and actionable auth content stays unavailable; informational login
//! alerts may be allowed. The human reader uses separate unrestricted methods.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use squelch_core::config::ShipmentListPolicy;
use squelch_core::error::CoreError;
use squelch_core::store::agent_triage::AgentTriageStore;
use squelch_core::store::{NewAuditEntry, SearchSort, SqliteStore, Store};
use squelch_core::triage::agent_config::RankingConfig;
use squelch_core::triage::decision::{
    EmailKind, MessageDestination, RecordProposal, SupportedTime, ThreadAttentionDecision,
};
use squelch_core::types::{
    AccountId, Disposition, SenderRule, ShipmentLeg, ShipmentOrder, ThreadView,
};

/// The squelch MCP server. Single-account: the account is resolved once at
/// construction, though every row already carries `account_id`.
#[derive(Clone)]
pub struct SquelchServer {
    store: Arc<SqliteStore>,
    account_id: AccountId,
    /// The operator's `[carriers]` LISTING policy, carried so `get_shipments`
    /// hides exactly what `GET /client/shipments` hides. Defaults to the config
    /// default; wire the real one with
    /// [`SquelchServer::with_shipment_policy`].
    ///
    /// This field exists because the agent door once hardcoded the BUILT-IN
    /// retirement cap and ignored the operator's, so an operator who set
    /// `max_failures = 1` kept seeing retired phantoms through their agent for
    /// four more failures, and one who set `10` had live packages hidden from it
    /// at five. Two doors, one view.
    shipment_policy: ShipmentListPolicy,
    ranking_config: RankingConfig,
    // Read only by the macro-generated `ServerHandler`, so dead-code analysis
    // can't see the use.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

/// Parameters for `get_inbox_updates`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetInboxUpdatesParams {
    /// Only return updates received at or after this UTC timestamp (RFC 3339).
    pub since: DateTime<Utc>,
    /// Deprecated compatibility argument. FYE membership is decided by the agent, not this score.
    #[serde(default)]
    pub min_importance: Option<u8>,
}

/// Parameters for `get_thread`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetThreadParams {
    /// The thread id to fetch.
    pub id: String,
}

/// Parameters for `search_mail`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchMailParams {
    /// Free-text query. Matched with hybrid keyword + semantic recall.
    pub query: String,
    /// Max number of summaries to return (1-50). Defaults to 10.
    #[serde(default)]
    pub k: Option<u8>,
    /// Result order. "recent" (the default) ranks by relevance with a tilt
    /// toward mail that arrived recently. "best_match" turns that tilt off and
    /// ranks on relevance alone — use it when the words matter more than the
    /// date, such as an old thread you can quote but cannot place.
    #[serde(default)]
    pub sort: Option<String>,
}

/// One `search_mail` result: a SUMMARY ONLY, never a body.
#[derive(Debug, serde::Serialize)]
pub struct SearchMailHit {
    /// Sender address (with display name when known).
    pub sender: String,
    /// A one-line summary — the message subject (never the body).
    pub one_line: String,
    pub received_at: DateTime<Utc>,
    /// The id to pass to `get_thread` to read the full thread.
    pub thread_id: String,
    /// Rank position (1 = best) in the fused hybrid search, under whichever
    /// `sort` ran. NOT "most textually similar" under the default sort, which
    /// blends recency in.
    pub relevance: u32,
}

/// Parameters for `get_deadlines`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDeadlinesParams {
    /// Only return deadlines due within this many days. Omit for all deadlines.
    #[serde(default)]
    pub within_days: Option<u32>,
}

/// Explicit due facts; a date-only source stays a date rather than invented midnight.
#[derive(Debug, serde::Serialize)]
pub struct DeadlineHit {
    pub id: i64,
    pub account_id: AccountId,
    pub message_id: i64,
    pub thread_id: String,
    pub kind: String,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub due_at: String,
    pub timezone: Option<String>,
    pub past_due: bool,
    pub source: String,
}

fn due_in_window(due: &SupportedTime, now: DateTime<Utc>, days: Option<u32>) -> Option<bool> {
    let cutoff =
        days.and_then(|days| now.checked_add_signed(chrono::Duration::days(i64::from(days))));
    if let Ok(at) = DateTime::parse_from_rfc3339(&due.value) {
        return cutoff.is_none_or(|cutoff| at <= cutoff).then_some(at < now);
    }
    let date = NaiveDate::parse_from_str(&due.value, "%Y-%m-%d").ok()?;
    cutoff
        .is_none_or(|cutoff| date <= cutoff.date_naive())
        .then_some(date < now.date_naive())
}

/// Parameters for `get_shipments`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetShipmentsParams {
    /// Include delivered shipments too. Omit/false => en-route packages only.
    #[serde(default)]
    pub include_delivered: Option<bool>,
}

/// A delivery fact with optional carrier observations, guarded by source provenance.
#[derive(Debug, serde::Serialize)]
pub struct ShipmentHit {
    pub item_name: String,
    pub carrier: String,
    pub status: String,
    pub tracking_number: String,
    pub tracking_url: Option<String>,
    pub last_update: DateTime<Utc>,
    /// Carrier-estimated delivery; `None` when the carrier gives none (and
    /// always `None` on a daemon that polls no carrier).
    pub eta: Option<DateTime<Utc>>,
    /// The carrier's own latest status string, verbatim; `None` until the first
    /// poll. Carried because our five-rung ladder loses detail the agent can
    /// usefully relay ("Delivered to neighbor", "Held at customs").
    pub carrier_status_raw: Option<String>,
    /// Who sold it, when known. Same meaning as the human door's.
    pub merchant: Option<String>,
    /// The orders this package (or, on a grouped hit, every package in the
    /// group) carries.
    pub orders: Vec<ShipmentOrder>,
    /// The other packages folded into this hit because they share an order.
    /// Grouped by the same rule as the human door, after the same hides.
    pub legs: Vec<ShipmentLeg>,
    /// The carrier row behind this hit, for grouping. Not served.
    #[serde(skip)]
    row: Option<squelch_core::types::Shipment>,
}

impl ShipmentHit {
    fn from_row(shipment: squelch_core::types::Shipment) -> Self {
        Self {
            item_name: shipment.item_name.clone(),
            carrier: shipment.carrier.clone(),
            status: shipment.status.clone(),
            tracking_number: shipment.tracking_number.clone(),
            tracking_url: shipment.tracking_url.clone(),
            last_update: shipment.last_update,
            eta: shipment.eta,
            carrier_status_raw: shipment.carrier_status_raw.clone(),
            merchant: shipment.merchant.clone(),
            orders: shipment.orders.clone(),
            legs: Vec::new(),
            row: Some(shipment),
        }
    }
}

/// One purchase, one hit: the human door's grouping (`order_link`), applied to
/// the agent door's merged list, with the same representative rule
/// ([`representative_first`](squelch_core::triage::order_link::representative_first):
/// the newest package still on its way, the newest overall only when all have
/// landed). Only a hit backed by a carrier row can carry orders, so a record
/// with no tracking number always stands alone. The name and merchant fall
/// back to the newest non-empty one in the group.
fn group_hits(hits: Vec<ShipmentHit>) -> Vec<ShipmentHit> {
    use squelch_core::triage::order_link::{card_name_and_merchant, fold_groups, union_orders};
    let row_id = |h: &ShipmentHit| h.row.as_ref().map_or(0, |r| r.id);
    let groups = fold_groups(
        hits,
        |h| h.orders.as_slice(),
        |h| (h.status == "delivered", h.last_update, row_id(h)),
    );
    let mut out = Vec::with_capacity(groups.len());
    for members in groups {
        let orders = union_orders(members.iter().map(|m| m.orders.as_slice()));
        let (item_name, merchant) = card_name_and_merchant(
            members
                .iter()
                .map(|m| (m.item_name.as_str(), m.merchant.as_deref())),
        );
        let mut members = members.into_iter();
        let Some(mut hit) = members.next() else {
            continue;
        };
        hit.legs = members
            .map(|m| ShipmentLeg {
                id: row_id(&m),
                delivered_at: m.row.as_ref().and_then(|r| r.delivered_at),
                carrier: m.carrier,
                tracking_number: m.tracking_number,
                status: m.status,
                tracking_url: m.tracking_url,
                last_update: m.last_update,
            })
            .collect();
        hit.item_name = item_name;
        hit.merchant = merchant;
        hit.orders = orders;
        out.push(hit);
    }
    out
}

/// Parameters for `set_sender_rule`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetSenderRuleParams {
    /// Address or pattern to match the sender against.
    pub match_pattern: String,
    /// Free-text description of what the user wants for this sender.
    pub want: String,
    /// One of: "surface", "squelch", "filtered".
    pub disposition: String,
}

/// The account owner's standing instruction for a sender, delivered ALONGSIDE
/// that sender's mail (issue #21).
///
/// This is a sender rule's `want_text`: the owner's own words, written through
/// the client or through `set_sender_rule`, which is audited fail-closed. It is
/// never text out of an email body. Stage-2 has read it since sender rules
/// shipped — it is how a `filtered` rule decides surface-vs-squelch — but a rule
/// that only steers triage is invisible to the agent doing the talking, so
/// "tell me the total on the statement, not the minimum payment" had no way to
/// reach the sentence the user actually reads.
///
/// DELIBERATELY NOT THE DISPOSITION. surface/squelch/filtered is a VERDICT, and
/// the pipeline has already applied it: by the time the agent sees a row, the
/// verdict is its `tier` and `importance`. Handing over the verdict too invites
/// the agent to apply it a second time, and there is one case where doing so is
/// actively wrong — a bill OUTRANKS a squelch rule on purpose (Rung 1 runs
/// before Rung 2 in `triage::stage1`), so an agent re-reading "squelch" off a
/// past-due notice would bury exactly the mail the ladder went out of its way to
/// raise. What travels is the instruction, not the judgment.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StandingInstruction {
    /// The rule's sender pattern, so the agent can attribute what it is
    /// following ("per your rule for `*@chase.com`") instead of asserting an
    /// unsourced preference at the user.
    pub match_pattern: String,
    /// The owner's instruction, VERBATIM — the same bytes Stage-2 reads. Either
    /// polarity: it may name what they want from this sender ("only the
    /// statement total") or what they do not care about ("skip the promos").
    pub want: String,
}

/// One `get_inbox_updates` result: the ranked update, plus the sender's standing
/// instruction when they have one.
///
/// A WRAPPER RATHER THAN A FIELD ON [`Update`]: `Update` is the human door's row
/// type as well, and the client reads rule text from its own rules list already.
/// The flatten leaves every existing key exactly where it was, so this is an
/// additive change to the agent door's wire shape.
#[derive(Debug, serde::Serialize)]
pub struct InboxUpdate {
    pub message_id: i64,
    pub thread_id: String,
    pub sender: String,
    pub received_at: String,
    pub summary: String,
    pub kinds: Vec<EmailKind>,
    pub destinations: Vec<MessageDestination>,
    pub attention: ThreadAttentionDecision,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standing_instruction: Option<StandingInstruction>,
}

/// A `get_thread` result: the sanitized thread, plus the standing instructions
/// covering the people in it.
///
/// A LIST, because a thread can have several correspondents and a rule can match
/// any of them. Deduplicated by pattern, so one rule matching four messages
/// reads once rather than four times, and absent from the JSON entirely when
/// nobody in the thread is ruled.
#[derive(Debug, serde::Serialize)]
pub struct ThreadWithInstructions {
    #[serde(flatten)]
    pub view: ThreadView,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub standing_instructions: Vec<StandingInstruction>,
}

impl SquelchServer {
    /// Build a server over an already-open store, resolving `account_email` to
    /// an account id (creating the account row if needed).
    pub fn new(store: Arc<SqliteStore>, account_email: &str) -> anyhow::Result<Self> {
        let account_id = store.ensure_account(account_email)?;
        Ok(Self {
            store,
            account_id,
            shipment_policy: ShipmentListPolicy::default(),
            ranking_config: RankingConfig::default(),
            tool_router: Self::tool_router(),
        })
    }

    /// Carry the operator's `[carriers]` listing policy into `get_shipments`.
    ///
    /// A BUILDER RATHER THAN AN ARGUMENT to [`SquelchServer::new`] deliberately,
    /// matching how `ApiState` takes its optional wiring: the default is a
    /// working server, and the daemon adds the configured value on the way past.
    /// Pass the SAME value you give `ApiState::with_shipment_policy` — the whole
    /// point is that the two doors agree on which packages exist.
    pub fn with_shipment_policy(mut self, policy: ShipmentListPolicy) -> Self {
        self.shipment_policy = policy;
        self
    }

    /// Map a core error onto the MCP wire. NotFound becomes `resource_not_found`;
    /// everything else becomes an opaque internal error (never leaks internals).
    fn map_err(e: CoreError) -> ErrorData {
        match e {
            CoreError::NotFound => ErrorData::resource_not_found("not found", None),
            CoreError::InvalidInput(m) => ErrorData::invalid_params(m, None),
            _ => ErrorData::internal_error("internal error", None),
        }
    }

    pub fn with_ranking_config(mut self, config: RankingConfig) -> Self {
        self.ranking_config = config;
        self
    }

    /// Pending, restricted, missing, and stale source assessments all block the
    /// thread. The store repeats this check inside the full-thread read lock.
    fn thread_is_unavailable(&self, thread_id: &str) -> Result<bool, ErrorData> {
        self.store
            .external_thread_allowed(self.account_id, thread_id)
            .map(|allowed| !allowed)
            .map_err(Self::map_err)
    }

    /// Number of registered MCP tools (for smoke tests / introspection).
    #[allow(dead_code)]
    pub fn tool_count(&self) -> usize {
        self.tool_router.list_all().len()
    }

    /// The account's sender rules, read in the order `squelch-core` itself reads
    /// them (`list_sender_rules`, newest-updated first). Same list, same order,
    /// so the rule this door names for an address is the rule Stage-1 would pick
    /// for it — [`squelch_core::triage::rules::match_sender_rule`] takes the
    /// FIRST match, which makes the order part of the answer whenever two
    /// patterns overlap.
    fn sender_rules(&self) -> Result<Vec<SenderRule>, ErrorData> {
        self.store
            .list_sender_rules(self.account_id)
            .map_err(Self::map_err)
    }

    /// The standing instruction for one sender, resolved BY ADDRESS against the
    /// rules as they stand NOW.
    ///
    /// NOT A JOIN ON `triage.matched_rule_id`, which is the obvious
    /// implementation and the wrong one, for three separate reasons:
    ///
    /// 1. THE MOTIVATING EXAMPLE WOULD MISS. `matched_rule_id` records the rule
    ///    that DECIDED the row, and Rung 1 (bill/payment) is evaluated BEFORE
    ///    sender rules and returns `matched_rule: None` — it reads the rule only
    ///    to decide whether to trust the sender's "past due". A credit-card
    ///    statement is a bill, so "tell me the total on cc statements" is
    ///    precisely the mail whose triage row carries no rule id at all.
    /// 2. A RULE WRITTEN AFTER THE MAIL LANDED leaves no mark on rows already
    ///    triaged — the same gap that made
    ///    [`squelch_core::triage::events::current_rule`] necessary for the
    ///    refine sites, and the common shape here: the user tells the agent what
    ///    they want *because* of the mail they were just shown.
    /// 3. The id answers "what decided this row". The question this door is
    ///    asking is "what did the owner ask for about this sender", which is a
    ///    property of the address, not of one verdict.
    ///
    /// IN RUST, NOT IN SQL. `match_sender_rule`'s globs are hand-rolled (`*`
    /// only, case-insensitive, nothing else metacharacter) exactly so a
    /// user-authored pattern cannot inject one. SQLite's `GLOB` is
    /// case-SENSITIVE and gives `?` and `[` meaning; `LIKE` reads `%` and `_`.
    /// Either re-expression would be a second spelling of a predicate that
    /// already exists in Rust, free to drift from the one triage actually ran.
    ///
    /// A BLANK `want_text` IS NO INSTRUCTION. Only a `filtered` rule is required
    /// to carry text (`validate_sender_rule`); a bare surface/squelch rule is a
    /// verdict with nothing to say about how to present anything, and an empty
    /// string on the wire reads as an instruction the agent has to interpret.
    /// Gated on `trim`, sent unmodified: `want_text` is stored verbatim and
    /// every other reader of it gets the owner's bytes.
    fn instruction_for(from_addr: &str, rules: &[SenderRule]) -> Option<StandingInstruction> {
        let rule = squelch_core::triage::rules::match_sender_rule(from_addr, rules)?;
        if rule.want_text.trim().is_empty() {
            return None;
        }
        Some(StandingInstruction {
            match_pattern: rule.match_pattern.clone(),
            want: rule.want_text.clone(),
        })
    }

    /// Every standing instruction covering a thread's correspondents, in
    /// first-appearance order, deduplicated by the pattern that produced it.
    ///
    /// Deduplicated by PATTERN rather than by address: two addresses under one
    /// `*@chase.com` rule are one instruction, and repeating it per message
    /// would read as two different asks.
    fn instructions_for_thread(
        view: &ThreadView,
        rules: &[SenderRule],
    ) -> Vec<StandingInstruction> {
        let mut out: Vec<StandingInstruction> = Vec::new();
        for m in &view.messages {
            if let Some(found) = Self::instruction_for(&m.from_addr, rules)
                && !out.iter().any(|e| e.match_pattern == found.match_pattern)
            {
                out.push(found);
            }
        }
        out
    }

    /// Serialize a thread with its senders' standing instructions attached. Both
    /// of `get_thread`'s resolution paths (thread id, message id) end here, so
    /// the instruction cannot ride on one and not the other.
    fn thread_with_instructions(&self, view: ThreadView) -> Result<CallToolResult, ErrorData> {
        let rules = self.sender_rules()?;
        let standing_instructions = Self::instructions_for_thread(&view, &rules);
        Self::ok_json(ThreadWithInstructions {
            view,
            standing_instructions,
        })
    }

    fn ok_json<T: serde::Serialize>(value: T) -> Result<CallToolResult, ErrorData> {
        let block = ContentBlock::json(value)?;
        Ok(CallToolResult::success(vec![block]))
    }
}

#[tool_router]
impl SquelchServer {
    /// Ranked inbox updates. Sealed rows are absent (never redacted).
    #[tool(
        name = "get_inbox_updates",
        description = "Ranked inbox updates since a timestamp. Each result's \
                       `thread_id` is the id to pass to get_thread to read the \
                       full thread. A result may carry `standing_instruction` — \
                       the account owner's own standing words about what they \
                       want from that sender ({match_pattern, want}). It is an \
                       instruction about WHAT TO REPORT, not a verdict about \
                       whether to report: consider it alongside the attention \
                       assessment and explain meaningful exceptions. \
                       Pending assessments and actionable authentication secrets are absent from \
                       results."
    )]
    async fn get_inbox_updates(
        &self,
        Parameters(params): Parameters<GetInboxUpdatesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let updates = self
            .store
            .external_agent_fye(self.account_id, 1000, &self.ranking_config, Utc::now())
            .map_err(Self::map_err)?;
        let rules = self.sender_rules()?;
        let mut out = Vec::new();
        for item in updates {
            let received_at = DateTime::parse_from_rfc3339(&item.received_at)
                .map_err(|_| ErrorData::internal_error("invalid received timestamp", None))?;
            if received_at.with_timezone(&Utc) < params.since
                || !self
                    .store
                    .agent_access_allowed(self.account_id, item.message_id)
                    .map_err(Self::map_err)?
                || self.thread_is_unavailable(&item.thread_id)?
            {
                continue;
            }
            out.push(InboxUpdate {
                standing_instruction: Self::instruction_for(&item.from_addr, &rules),
                message_id: item.message_id,
                thread_id: item.thread_id,
                sender: item.from_addr,
                received_at: item.received_at,
                summary: item.decision.summary,
                kinds: item.decision.kinds,
                destinations: item.decision.destinations,
                attention: item.attention,
                score: item.score,
            });
        }
        // Returning data to an agent is not a human opening or resolving mail.
        Self::ok_json(out)
    }

    /// Full sanitized thread view. `id` may be a thread id or a single message
    /// id. A sealed id and a nonexistent one return the same `resource_not_found`
    /// through BOTH paths, so a sealed message's existence cannot be inferred.
    #[tool(
        name = "get_thread",
        description = "Fetch a sanitized thread. `id` is EITHER a thread id (the \
                       `thread_id` field returned by get_inbox_updates and \
                       search_mail) OR a single message id — a message id resolves \
                       to its thread. `standing_instructions` carries the account \
                       owner's own standing words about the people in the thread \
                       ({match_pattern, want}); follow them when you report what \
                       the thread says. Unknown, pending, or restricted ids return an \
                       identical not-found error."
    )]
    async fn get_thread(
        &self,
        Parameters(params): Parameters<GetThreadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // PATH 1: treat `id` as a thread id.
        match self.store.thread_view(self.account_id, &params.id) {
            Ok(view) => self.thread_with_instructions(view),
            Err(CoreError::NotFound) => {
                // PATH 2: retry `id` as a MESSAGE id. `thread_id_for_message`
                // excludes sealed rows in SQL, so a sealed or nonexistent message
                // id both yield None -> the identical 404.
                let message_id: i64 = match params.id.parse() {
                    Ok(n) => n,
                    // Not numeric => can't be a message id; keep the same 404.
                    Err(_) => return Err(ErrorData::resource_not_found("not found", None)),
                };
                let thread_id = self
                    .store
                    .thread_id_for_message(self.account_id, message_id)
                    .map_err(Self::map_err)?;
                let Some(thread_id) = thread_id else {
                    return Err(ErrorData::resource_not_found("not found", None));
                };
                // Re-guard the resolved thread: an unsealed message may have a
                // sealed sibling, which seals the whole thread.
                if self.thread_is_unavailable(&thread_id)? {
                    return Err(ErrorData::resource_not_found("not found", None));
                }
                let view: ThreadView = self
                    .store
                    .thread_view(self.account_id, &thread_id)
                    .map_err(Self::map_err)?;
                self.thread_with_instructions(view)
            }
            Err(e) => Err(Self::map_err(e)),
        }
    }

    /// Explicit bill due dates and unresolved attention deadlines.
    #[tool(
        name = "get_deadlines",
        description = "Agent-assessed bill due dates and unresolved action deadlines within N days (default all). Date-only facts remain dates; no importance threshold."
    )]
    async fn get_deadlines(
        &self,
        Parameters(params): Parameters<GetDeadlinesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let now = Utc::now();
        let records = self
            .store
            .external_agent_records_with_query(
                self.account_id,
                usize::MAX,
                &squelch_core::store::agent_triage::AgentListQuery {
                    since: None,
                    include_done: false,
                },
            )
            .map_err(Self::map_err)?;
        let mut out = Vec::new();
        for item in records {
            for record in item.decision.records {
                if let RecordProposal::Bill {
                    merchant,
                    amount,
                    currency,
                    due: Some(due),
                    ..
                } = record
                    && let Some(past_due) = due_in_window(&due, now, params.within_days)
                {
                    out.push(DeadlineHit {
                        id: item.message_id,
                        account_id: self.account_id,
                        message_id: item.message_id,
                        thread_id: item.thread_id.clone(),
                        kind: "bill".into(),
                        amount,
                        currency,
                        due_at: due.value,
                        timezone: due.timezone,
                        past_due,
                        source: merchant,
                    });
                }
            }
        }
        let attention = self
            .store
            .external_agent_fye(self.account_id, usize::MAX, &self.ranking_config, now)
            .map_err(Self::map_err)?;
        for item in attention {
            let mut times: Vec<(&SupportedTime, &str)> = item
                .attention
                .actions
                .iter()
                .filter(|action| !action.resolved)
                .filter_map(|action| {
                    action
                        .due
                        .as_ref()
                        .map(|due| (due, action.description.as_str()))
                })
                .collect();
            if times.is_empty()
                && let Some(due) = &item.attention.factors.attention_at
            {
                times.push((due, &item.attention.summary));
            }
            for (due, description) in times {
                if out
                    .iter()
                    .any(|row| row.thread_id == item.thread_id && row.due_at == due.value)
                {
                    continue;
                }
                if let Some(past_due) = due_in_window(due, now, params.within_days) {
                    out.push(DeadlineHit {
                        id: item.message_id,
                        account_id: self.account_id,
                        message_id: item.message_id,
                        thread_id: item.thread_id.clone(),
                        kind: "action".into(),
                        amount: None,
                        currency: None,
                        due_at: due.value.clone(),
                        timezone: due.timezone.clone(),
                        past_due,
                        source: description.into(),
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            a.due_at
                .cmp(&b.due_at)
                .then_with(|| a.message_id.cmp(&b.message_id))
        });
        Self::ok_json(out)
    }

    /// Model-proposed deliveries, enriched by retained carrier observations.
    #[tool(
        name = "get_shipments",
        description = "Agent-assessed deliveries and tracked packages, including deliveries without a tracking number. Delivered packages are omitted unless include_delivered=true. Carrier observations enrich known tracking numbers; unavailable tracking details remain empty/null."
    )]
    async fn get_shipments(
        &self,
        Parameters(params): Parameters<GetShipmentsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let include_delivered = params.include_delivered.unwrap_or(false);
        let records = self
            .store
            .external_agent_records_with_query(
                self.account_id,
                usize::MAX,
                &squelch_core::store::agent_triage::AgentListQuery {
                    since: None,
                    include_done: false,
                },
            )
            .map_err(Self::map_err)?;
        // The observation feed, UNGROUPED and not yet judged silent: a record
        // hit is decorated with its carrier row first and judged after, so the
        // same `Silence::hides` the human door applies sees the same carrier
        // evidence here. Two doors, one rule, one package list.
        //
        // Only the rows the window is CERTAIN to hide are cut, in SQL (a long
        // delivered history stays on disk), and a record whose row was cut is
        // dropped by `agent_shipment_is_hidden` below, exactly as the row's
        // own `Silence::hides` would have dropped it.
        let silence = self.shipment_policy.silence(Utc::now());
        let shipments = self
            .store
            .external_shipments_within(self.account_id, silence)
            .map_err(Self::map_err)?;
        let mut out: Vec<ShipmentHit> = Vec::new();
        let mut represented = std::collections::HashSet::new();
        for item in records {
            for record in item.decision.records {
                let RecordProposal::Delivery {
                    carrier,
                    tracking_number,
                    status,
                    ..
                } = record
                else {
                    continue;
                };
                let number = tracking_number
                    .map(|raw| {
                        squelch_core::triage::extract::shipments::sanitize_tracking_number(
                            Some(&raw),
                            None,
                        )
                        .unwrap_or(raw)
                    })
                    .unwrap_or_default();
                if !number.is_empty() && !represented.insert(number.clone()) {
                    continue;
                }
                if !number.is_empty()
                    && self
                        .store
                        .agent_shipment_is_hidden(self.account_id, &number, silence)
                        .map_err(Self::map_err)?
                {
                    continue;
                }
                let mut hit = ShipmentHit {
                    item_name: item.decision.summary.clone(),
                    merchant: None,
                    orders: Vec::new(),
                    legs: Vec::new(),
                    row: None,
                    carrier: carrier.unwrap_or_default(),
                    status,
                    tracking_number: number,
                    tracking_url: None,
                    last_update: DateTime::parse_from_rfc3339(&item.received_at)
                        .map_err(|error| Self::map_err(CoreError::Other(error.into())))?
                        .with_timezone(&Utc),
                    eta: None,
                    carrier_status_raw: None,
                };
                let observation = shipments
                    .iter()
                    .find(|shipment| shipment.tracking_number == hit.tracking_number);
                if let Some(observation) = observation {
                    hit.tracking_url = observation.tracking_url.clone();
                    hit.eta = observation.eta;
                    hit.carrier_status_raw = observation.carrier_status_raw.clone();
                    // The row's name, merchant and orders are the reconciled
                    // ones (sanitized, newest first); the summary is only the
                    // fallback for a row that has no name.
                    if !observation.item_name.trim().is_empty() {
                        hit.item_name = observation.item_name.clone();
                    }
                    hit.merchant = observation.merchant.clone();
                    hit.orders = observation.orders.clone();
                    hit.row = Some(observation.clone());
                    // THE ROW'S STATUS IS THE PACKAGE'S STATUS, not this
                    // record's. Reconcile already decided it from every
                    // retained proposal plus the carrier: the newest proposal
                    // on an owned row, the no-regress merge on a legacy one
                    // (a delivered legacy row stays delivered when newer mail
                    // says "shipped"). Reading the record here instead made the
                    // doors pick different representatives, and disagree about
                    // which cards `include_delivered=false` drops.
                    hit.status = observation.status.clone();
                    // THE ROW'S CLOCK IS THE PACKAGE'S CLOCK. Reconcile already
                    // folded every accepted mail into `last_update`; a mail it
                    // rejected (no carrier, a status it could not read) is not
                    // news on the human door and must not be news here, or the
                    // doors disagree about exactly the hidden rows.
                    hit.last_update = observation.last_update;
                }
                // NO delivered filter here: grouping sees every visible
                // package, exactly as the human door's does, and the filter
                // applies to whole cards below.
                if !silence.is_some_and(|s| s.hides(hit.last_update, observation)) {
                    out.push(hit);
                }
            }
        }
        // Legacy tracked packages keep their carrier observations during cutover.
        for shipment in shipments {
            if represented.contains(&shipment.tracking_number)
                || silence.is_some_and(|s| s.hides(shipment.last_update, Some(&shipment)))
            {
                continue;
            }
            out.push(ShipmentHit::from_row(shipment));
        }
        out.sort_by(|a, b| b.last_update.cmp(&a.last_update));
        // A card is delivered iff its representative is, which is only when
        // every package on it has landed.
        let cards: Vec<ShipmentHit> = group_hits(out)
            .into_iter()
            .filter(|hit| include_delivered || hit.status != "delivered")
            .collect();
        Self::ok_json(cards)
    }

    /// Create or update a local sender rule. Writes ONLY squelch's local store;
    /// never touches Gmail.
    #[tool(
        name = "set_sender_rule",
        description = "Create/update a LOCAL sender rule (surface|squelch|filtered). \
                       Writes only squelch's local store, never the mailbox."
    )]
    async fn set_sender_rule(
        &self,
        Parameters(params): Parameters<SetSenderRuleParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let disposition = Disposition::parse(&params.disposition).ok_or_else(|| {
            ErrorData::invalid_params(
                "disposition must be one of: surface, squelch, filtered",
                None,
            )
        })?;

        // AUDIT (agent door): this is the highest-value entry in the ledger — a
        // prompt-injected agent tampering with rules is the known blast radius of
        // this tool, and it must never write untraced. detail carries the
        // disposition + the `want` text truncated to ~120 chars so the human
        // review UI reads cleanly without unbounded free text.
        let detail = format!(
            "{}: {}",
            disposition.as_str(),
            squelch_core::text::truncate_ellipsis(&params.want, 120)
        );
        let audit = NewAuditEntry {
            actor: "agent".to_string(),
            action: "rule.set".to_string(),
            target: Some(params.match_pattern.clone()),
            detail: Some(detail),
        };

        // FAIL-CLOSED: the audit row is committed in the SAME transaction as the
        // rule write. If the audit insert fails, the rule write is rolled back and
        // the tool returns an error — stricter than the human door's best-effort
        // action audit, because this is a WRITE by an untrusted-adjacent actor.
        let id = self
            .store
            .set_sender_rule_audited(
                self.account_id,
                &params.match_pattern,
                &params.want,
                disposition,
                &audit,
            )
            .map_err(Self::map_err)?;
        Self::ok_json(serde_json::json!({ "rule_id": id }))
    }

    /// List local sender rules for the active account.
    #[tool(
        name = "list_sender_rules",
        description = "List the local sender rules for this account."
    )]
    async fn list_sender_rules(&self) -> Result<CallToolResult, ErrorData> {
        let rules = self
            .store
            .list_sender_rules(self.account_id)
            .map_err(Self::map_err)?;
        Self::ok_json(rules)
    }

    /// Hybrid keyword + semantic search over the mailbox. Returns SUMMARIES ONLY
    /// (sender, subject one-line, received_at, thread_id, relevance) — never
    /// bodies. `get_thread` remains the escalation to read full content: pass a
    /// result's `thread_id` to it.
    ///
    /// RECENCY IS PART OF THE RANK, and the agent can turn it off. The default
    /// order tilts toward mail that landed recently, so `relevance: 1` means
    /// "best answer", not "most textually similar". An agent hunting an OLD
    /// thread passes `sort: "best_match"` rather than reading down the list.
    ///
    /// SEALED: auth/verification mail is never embedded and is excluded in SQL by
    /// both the keyword and semantic legs, so it can never appear here. A
    /// defense-in-depth re-check drops any hit whose thread overlaps a sealed
    /// thread before serialization, mirroring `get_inbox_updates`.
    #[tool(
        name = "search_mail",
        description = "Search the mailbox (hybrid keyword + semantic recall). \
                       Ranked with a tilt toward recent mail by default; pass \
                       sort=\"best_match\" to rank on relevance alone when \
                       hunting an older thread. Returns SUMMARIES ONLY (sender, \
                       one-line subject, received_at, thread_id, relevance) — \
                       never message bodies. To read a result, pass its \
                       `thread_id` to get_thread. Auth/verification emails are \
                       structurally absent."
    )]
    async fn search_mail(
        &self,
        Parameters(params): Parameters<SearchMailParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let query = params.query.trim();
        if query.is_empty() {
            return Err(ErrorData::invalid_params("query must not be empty", None));
        }
        // Default 10, clamp to 1..=50 (u8 default `10` when omitted).
        let k = params.k.unwrap_or(10).clamp(1, 50) as usize;

        // An unreadable sort is the agent's mistake to see, not one to paper
        // over: silently serving `recent` for a `sort` the model invented would
        // teach it that the argument works.
        let sort = match params.sort.as_deref() {
            Some(s) => SearchSort::parse(s).ok_or_else(|| {
                ErrorData::invalid_params("sort must be one of: recent, best_match", None)
            })?,
            None => SearchSort::default(),
        };

        // Search expands its recall window until access filtering leaves the
        // requested number of readable results, or the corpus is exhausted.
        let store = self.store.clone();
        let account_id = self.account_id;
        let query = query.to_string();
        let hits =
            tokio::task::spawn_blocking(move || store.external_search(account_id, &query, sort, k))
                .await
                .map_err(|_| ErrorData::internal_error("internal error", None))?
                .map_err(Self::map_err)?;

        let mut out = Vec::with_capacity(hits.len());
        for hit in hits {
            let sender = match &hit.from_name {
                Some(name) if !name.trim().is_empty() => {
                    format!("{} <{}>", name.trim(), hit.from_addr)
                }
                _ => hit.from_addr.clone(),
            };
            out.push(SearchMailHit {
                sender,
                // one_line is the SUBJECT — a summary, never the body.
                one_line: hit.subject,
                received_at: hit.received_at,
                thread_id: hit.thread_id,
                relevance: (out.len() as u32) + 1,
            });
        }
        Self::ok_json(out)
    }
}

#[tool_handler]
impl ServerHandler for SquelchServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "squelch: local-first email intelligence. Read-only over your \
                 mailbox; the only writes are local sender rules. Use search_mail \
                 to find mail (summaries only) and get_thread to read a thread — \
                 pass a result's thread_id (get_thread also accepts a message id). \
                 get_deadlines lists bills due; get_shipments lists packages in \
                 transit. When mail arrives from a sender the account owner has \
                 written a rule for, get_inbox_updates and get_thread deliver \
                 that rule's instruction text with it — obey it when you report \
                 that mail. Pending assessments and actionable auth secrets are \
                 unavailable. Informational login and security alerts may be readable.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::wrapper::Parameters;
    use squelch_core::store::Store;

    #[tokio::test]
    async fn set_sender_rule_writes_audit_row() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let acct = store.ensure_account("me@localhost").unwrap();
        let server = SquelchServer::new(store.clone(), "me@localhost").unwrap();

        let long_want = "x".repeat(200);
        let res = server
            .set_sender_rule(Parameters(SetSenderRuleParams {
                match_pattern: "*@spam.com".into(),
                want: long_want,
                disposition: "squelch".into(),
            }))
            .await
            .unwrap();
        assert!(!res.is_error.unwrap_or(false));

        // The rule landed...
        let rules = store.list_sender_rules(acct).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].match_pattern, "*@spam.com");

        // ...and so did exactly one audit row with the expected shape.
        let audit = store.list_audit(acct, 10).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].actor, "agent");
        assert_eq!(audit[0].action, "rule.set");
        assert_eq!(audit[0].target.as_deref(), Some("*@spam.com"));
        let detail = audit[0].detail.as_deref().unwrap();
        assert!(detail.starts_with("squelch: "), "detail: {detail}");
        // want was truncated (200 chars -> ~120 + ellipsis), so far under the raw.
        assert!(detail.chars().count() <= 132, "detail too long: {detail}");
        assert!(detail.ends_with('…'), "truncation marker missing: {detail}");
    }
    #[tokio::test]
    async fn set_sender_rule_bad_disposition_writes_nothing() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let acct = store.ensure_account("me@localhost").unwrap();
        let server = SquelchServer::new(store.clone(), "me@localhost").unwrap();

        let err = server
            .set_sender_rule(Parameters(SetSenderRuleParams {
                match_pattern: "*@spam.com".into(),
                want: "nope".into(),
                disposition: "bogus".into(),
            }))
            .await;
        assert!(err.is_err());
        assert_eq!(store.list_sender_rules(acct).unwrap().len(), 0);
        assert_eq!(store.list_audit(acct, 10).unwrap().len(), 0);
    }

    fn seed(store: &SqliteStore, account: i64, thread: &str, text: &str) -> i64 {
        use squelch_core::sync::ingest::{RawFetched, ingest_with_rules};
        let now = Utc::now();
        let raw = RawFetched {
            account_id: account,
            gmail_msg_id: format!("{thread}-{text}"),
            gmail_thread_id: Some(thread.into()),
            raw: format!("From: alerts@example.com\r\nTo: me@localhost\r\nSubject: {text}\r\nDate: {}\r\n\r\n{text}", now.to_rfc2822()).into_bytes(),
            internal_date: Some(now), is_sent: false, is_spam: false,
            account_addr: "me@localhost".into(),
        };
        let message = ingest_with_rules(&raw, &Default::default(), now, &[], |_| false);
        store.ingest_message(&message).unwrap()
    }

    fn decide(
        store: &SqliteStore,
        account: i64,
        id: i64,
        restricted: bool,
        extra_source: Option<i64>,
    ) {
        use squelch_core::store::agent_triage::AgentCommitOutcome;
        use squelch_core::triage::decision::MessageDecision;
        store
            .enqueue_agent_triage(account, id, "test", false)
            .unwrap();
        // Ingest may have already queued an arrival; finish all jobs for this message.
        while let Some(job) = store
            .claim_agent_job(account, "triage", Utc::now(), 120)
            .unwrap()
        {
            assert_eq!(
                job.message_id, id,
                "test fixtures assess one message at a time"
            );
            let context = store.load_agent_context(&job).unwrap();
            let mut decision = MessageDecision {
                summary: "A meaningful update".into(),
                ..Default::default()
            };
            decision.external_access.restricted = restricted;
            decision.attention.show_in_fye = true;
            decision.attention.summary = "An update worth reading".into();
            decision.attention.relevant_message_ids = vec![id];
            let mut sources = vec![context.message.source.clone()];
            if let Some(source) = extra_source {
                sources.push(store.agent_read_message(account, source).unwrap().source);
            }
            assert_eq!(
                store
                    .commit_agent_decision(&job, &context, &decision, &sources)
                    .unwrap(),
                AgentCommitOutcome::Applied
            );
        }
    }

    #[tokio::test]
    async fn canonical_bill_and_delivery_reach_existing_tools_with_date_precision_and_guards() {
        use squelch_core::store::agent_triage::AgentCommitOutcome;
        use squelch_core::triage::decision::MessageDecision;
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let server = SquelchServer::new(store.clone(), "me@localhost").unwrap();
        let id = seed(&store, account, "records", "Bill and shipping confirmation");
        let job = store
            .claim_agent_job(account, "triage", Utc::now(), 60)
            .unwrap()
            .unwrap();
        let context = store.load_agent_context(&job).unwrap();
        let due = (Utc::now() + chrono::Duration::days(2))
            .format("%Y-%m-%d")
            .to_string();
        let decision = MessageDecision {
            summary: "Order and payment details".into(),
            destinations: vec![],
            records: vec![
                RecordProposal::Bill {
                    merchant: "Shop".into(),
                    amount: Some(42.0),
                    currency: Some("USD".into()),
                    due: Some(SupportedTime {
                        value: due.clone(),
                        source_message_id: id,
                        ..Default::default()
                    }),
                    autopay: None,
                    evidence: vec![],
                },
                RecordProposal::Delivery {
                    carrier: Some("ups".into()),
                    tracking_number: Some(" 1z999 aa10 1234 56784 ".into()),
                    status: "shipped".into(),
                    item_name: None,
                    merchant: None,
                    order_refs: vec![],
                    evidence: vec![],
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            store
                .commit_agent_decision(
                    &job,
                    &context,
                    &decision,
                    std::slice::from_ref(&context.message.source)
                )
                .unwrap(),
            AgentCommitOutcome::Applied
        );
        let bills = values(
            &server
                .get_deadlines(Parameters(GetDeadlinesParams {
                    within_days: Some(3),
                }))
                .await
                .unwrap(),
        );
        assert_eq!(bills.as_array().unwrap().len(), 1);
        assert_eq!(bills[0]["due_at"], due, "date-only facts remain date-only");
        assert_eq!(bills[0]["amount"], 42.0);
        assert!(
            values(
                &server
                    .get_deadlines(Parameters(GetDeadlinesParams {
                        within_days: Some(1)
                    }))
                    .await
                    .unwrap()
            )
            .as_array()
            .unwrap()
            .is_empty()
        );
        let polling_row = store.external_shipments(account, true).unwrap().remove(0);
        assert_eq!(polling_row.tracking_number, "1Z999AA10123456784");
        store
            .apply_carrier_track(
                account,
                polling_row.id,
                &squelch_core::triage::CarrierTrack {
                    status: Some(squelch_core::triage::ShipmentStatus::OutForDelivery),
                    carrier_status_raw: "Out for delivery".into(),
                    eta: Some(Utc::now() + chrono::Duration::days(1)),
                    delivered_at: None,
                },
                Utc::now(),
            )
            .unwrap();
        let deliveries = values(
            &server
                .get_shipments(Parameters(GetShipmentsParams {
                    include_delivered: None,
                }))
                .await
                .unwrap(),
        );
        assert_eq!(deliveries.as_array().unwrap().len(), 1);
        assert_eq!(deliveries[0]["tracking_number"], "1Z999AA10123456784");
        assert_eq!(deliveries[0]["status"], "out_for_delivery");
        assert_eq!(deliveries[0]["carrier_status_raw"], "Out for delivery");
        assert!(
            !deliveries[0]["eta"].is_null(),
            "spaced canonical number receives compact-row carrier observations"
        );
        store
            .clear_shipment(
                account,
                polling_row.id,
                Utc::now(),
                ShipmentListPolicy::default(),
            )
            .unwrap();
        assert!(
            values(
                &server
                    .get_shipments(Parameters(GetShipmentsParams {
                        include_delivered: Some(true)
                    }))
                    .await
                    .unwrap()
            )
            .as_array()
            .unwrap()
            .is_empty(),
            "canonical delivery cannot resurrect an explicitly cleared tracking number"
        );
        store
            .correct_agent_triage(
                account,
                id,
                "external_access",
                &serde_json::json!(true),
                Utc::now(),
            )
            .unwrap();
        assert!(
            values(
                &server
                    .get_deadlines(Parameters(GetDeadlinesParams { within_days: None }))
                    .await
                    .unwrap()
            )
            .as_array()
            .unwrap()
            .is_empty()
        );
        assert!(
            values(
                &server
                    .get_shipments(Parameters(GetShipmentsParams {
                        include_delivered: Some(true)
                    }))
                    .await
                    .unwrap()
            )
            .as_array()
            .unwrap()
            .is_empty()
        );
    }

    /// ONE PURCHASE, ONE HIT, on both doors. Two tracking numbers that share an
    /// order (the supplier leg and the leg to the user) come back from
    /// `get_shipments` as one hit shaped like the human door's one card: the
    /// newest package represents, the other is a leg, the name and merchant
    /// are the reconciled ones, and the orders are the union.
    #[tokio::test]
    async fn get_shipments_groups_one_order_like_the_human_door() {
        use squelch_core::store::agent_triage::AgentCommitOutcome;
        use squelch_core::triage::decision::MessageDecision;
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let policy = ShipmentListPolicy::default();
        let server = SquelchServer::new(store.clone(), "me@localhost")
            .unwrap()
            .with_shipment_policy(policy);
        let leg = |number: &str, item: Option<&str>, refs: &[&str]| MessageDecision {
            summary: "Shipment update".into(),
            records: vec![RecordProposal::Delivery {
                carrier: Some("ups".into()),
                tracking_number: Some(number.into()),
                status: "shipped".into(),
                item_name: item.map(str::to_string),
                merchant: Some("Bill's Exhausts".into()),
                order_refs: refs.iter().map(|r| r.to_string()).collect(),
                evidence: vec![],
            }],
            ..Default::default()
        };
        for (thread, decision) in [
            ("inbound", leg("1ZW061R3DG21045729", None, &["21470"])),
            (
                "outbound",
                leg(
                    "1ZB8B2560323528551",
                    Some("Austin Racing DB Killer AUR10"),
                    &["#21470"],
                ),
            ),
        ] {
            let id = seed(&store, account, thread, "Shipment update");
            while let Some(job) = store
                .claim_agent_job(account, "triage", Utc::now(), 120)
                .unwrap()
            {
                assert_eq!(job.message_id, id);
                let context = store.load_agent_context(&job).unwrap();
                assert_eq!(
                    store
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
        }
        assert_eq!(
            store.external_shipments(account, true).unwrap().len(),
            2,
            "the raw observation feed stays ungrouped"
        );
        let human = store.list_shipments(account, true, policy).unwrap();
        let agent = values(
            &server
                .get_shipments(Parameters(GetShipmentsParams {
                    include_delivered: Some(true),
                }))
                .await
                .unwrap(),
        );
        let agent = agent.as_array().unwrap();
        assert_eq!((human.len(), agent.len()), (1, 1), "one card on both doors");
        let hit = &agent[0];
        assert_eq!(hit["tracking_number"], human[0].tracking_number);
        assert_eq!(hit["item_name"], "Austin Racing DB Killer AUR10");
        assert_eq!(hit["item_name"], human[0].item_name);
        assert_eq!(hit["merchant"], "Bill's Exhausts");
        assert_eq!(hit["orders"].as_array().unwrap().len(), 1);
        assert_eq!(hit["orders"][0]["order_ref"], "21470");
        assert_eq!(hit["legs"].as_array().unwrap().len(), 1);
        assert_eq!(
            hit["legs"][0]["tracking_number"],
            human[0].legs[0].tracking_number
        );
        assert_eq!(hit["legs"][0]["id"], human[0].legs[0].id);
        assert!(
            hit.get("row").is_none(),
            "the grouping handle is not served"
        );
    }

    /// Ingest one mail received at `at` and commit one delivery record for
    /// it, the way production does. Returns the message id.
    #[allow(clippy::too_many_arguments)]
    fn deliver(
        store: &SqliteStore,
        account: i64,
        thread: &str,
        at: chrono::DateTime<Utc>,
        restricted: bool,
        number: &str,
        status: &str,
        merchant: Option<&str>,
        refs: &[&str],
    ) -> i64 {
        let record = RecordProposal::Delivery {
            carrier: Some("ups".into()),
            tracking_number: Some(number.into()),
            status: status.into(),
            item_name: None,
            merchant: merchant.map(str::to_string),
            order_refs: refs.iter().map(|r| r.to_string()).collect(),
            evidence: vec![],
        };
        ingest_decided(store, account, thread, at, restricted, number, vec![record])
    }

    /// Ingest one mail received at `at` (gmail id `{thread}-{key}`) and commit
    /// a decision carrying `records` for it. Returns the message id.
    fn ingest_decided(
        store: &SqliteStore,
        account: i64,
        thread: &str,
        at: chrono::DateTime<Utc>,
        restricted: bool,
        key: &str,
        records: Vec<RecordProposal>,
    ) -> i64 {
        use squelch_core::store::agent_triage::AgentCommitOutcome;
        use squelch_core::sync::ingest::{RawFetched, ingest_with_rules};
        use squelch_core::triage::decision::MessageDecision;
        let raw = RawFetched {
            account_id: account,
            gmail_msg_id: format!("{thread}-{key}"),
            gmail_thread_id: Some(thread.into()),
            raw: format!(
                "From: shop@example.com\r\nTo: me@localhost\r\nSubject: Update\r\nDate: {}\r\n\r\nUpdate",
                at.to_rfc2822()
            )
            .into_bytes(),
            internal_date: Some(at),
            is_sent: false,
            is_spam: false,
            account_addr: "me@localhost".into(),
        };
        let message = ingest_with_rules(&raw, &Default::default(), at, &[], |_| false);
        let id = store.ingest_message(&message).unwrap();
        let mut decision = MessageDecision {
            summary: "Shipment update".into(),
            records,
            ..Default::default()
        };
        decision.external_access.restricted = restricted;
        while let Some(job) = store
            .claim_agent_job(account, "triage", Utc::now(), 120)
            .unwrap()
        {
            assert_eq!(job.message_id, id);
            let context = store.load_agent_context(&job).unwrap();
            assert_eq!(
                store
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
        id
    }

    async fn agent_hits(server: &SquelchServer, include_delivered: bool) -> serde_json::Value {
        values(
            &server
                .get_shipments(Parameters(GetShipmentsParams {
                    include_delivered: Some(include_delivered),
                }))
                .await
                .unwrap(),
        )
    }

    /// A DELIVERED BOX NEVER STANDS FOR ONE STILL COMING, on either door, and
    /// both doors group the same packages whatever `include_delivered` says:
    /// the newer box landed two days ago, the older is still in transit, and
    /// both show one in-transit card carrying the delivered box as a leg.
    #[tokio::test]
    async fn both_doors_show_an_order_in_transit_while_any_box_is() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let policy = ShipmentListPolicy::default();
        let server = SquelchServer::new(store.clone(), "me@localhost")
            .unwrap()
            .with_shipment_policy(policy);
        let days = |n| Utc::now() - chrono::Duration::days(n);
        let (coming, landed) = ("1ZW061R3DG21045729", "1ZB8B2560323528551");
        deliver(
            &store,
            account,
            "a",
            days(3),
            false,
            coming,
            "shipped",
            Some("Bean Co"),
            &["5"],
        );
        deliver(
            &store,
            account,
            "b",
            days(2),
            false,
            landed,
            "delivered",
            Some("Bean Co"),
            &["5"],
        );
        for include_delivered in [true, false] {
            let human = store
                .list_shipments(account, include_delivered, policy)
                .unwrap();
            let agent = agent_hits(&server, include_delivered).await;
            let agent = agent.as_array().unwrap();
            assert_eq!((human.len(), agent.len()), (1, 1), "{include_delivered}");
            assert_eq!(human[0].tracking_number, coming);
            assert_eq!(human[0].status, "shipped");
            assert_eq!(agent[0]["tracking_number"], coming);
            assert_eq!(agent[0]["status"], "shipped");
            assert_eq!(human[0].legs.len(), 1, "{include_delivered}");
            assert_eq!(
                agent[0]["legs"].as_array().unwrap().len(),
                1,
                "the agent door groups the same members ({include_delivered})"
            );
        }
        // Every box landed: the card is delivered, and only a listing that
        // asks for delivered packages shows it, on either door.
        deliver(
            &store,
            account,
            "c",
            days(1),
            false,
            coming,
            "delivered",
            Some("Bean Co"),
            &["5"],
        );
        for (include_delivered, want) in [(true, 1), (false, 0)] {
            let human = store
                .list_shipments(account, include_delivered, policy)
                .unwrap();
            let agent = agent_hits(&server, include_delivered).await;
            assert_eq!(
                (human.len(), agent.as_array().unwrap().len()),
                (want, want),
                "{include_delivered}"
            );
        }
    }

    /// A LEGACY ROW'S STATUS IS THE ROW'S, on both doors. The old extractor
    /// recorded box X delivered; newer mail the model read as "shipped" (a
    /// re-ship notice, a survey) cannot walk a legacy row back, so the row
    /// stays delivered. The agent door used to serve the record's "shipped"
    /// instead, so it picked X to represent the order and kept the card under
    /// `include_delivered=false`; the human door picked box Y, still coming.
    #[tokio::test]
    async fn both_doors_take_a_legacy_rows_status_from_the_row() {
        use squelch_core::triage::ShipmentStatus;
        use squelch_core::triage::extract::shipments::ShipmentsApplied;
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let policy = ShipmentListPolicy::default();
        let server = SquelchServer::new(store.clone(), "me@localhost")
            .unwrap()
            .with_shipment_policy(policy);
        let days = |n| Utc::now() - chrono::Duration::days(n);
        let (legacy, coming) = ("1ZW061R3DG21045729", "1ZB8B2560323528551");
        // The legacy row: the old extractor's delivered verdict on an allowed
        // mail that carries no delivery record of its own.
        let minted = ingest_decided(&store, account, "old", days(5), false, "old", vec![]);
        assert!(
            store
                .shipments_extract_apply(&ShipmentsApplied {
                    message_id: minted,
                    account_id: account,
                    thread_id: "old".into(),
                    is_shipment: true,
                    tracking_number: Some(legacy.into()),
                    order_ref: None,
                    item_name: None,
                    carrier: "ups".into(),
                    status: Some(ShipmentStatus::Delivered),
                    received_at: days(5),
                    extractor_model_used: "legacy-extractor".into(),
                })
                .unwrap()
        );
        deliver(
            &store,
            account,
            "y",
            days(3),
            false,
            coming,
            "shipped",
            Some("Bean Co"),
            &["5"],
        );
        // Newer mail about the legacy box, read as "shipped", naming the order.
        deliver(
            &store,
            account,
            "x",
            days(1),
            false,
            legacy,
            "shipped",
            Some("Bean Co"),
            &["5"],
        );
        let raw = store.external_shipments(account, true).unwrap();
        let row = raw.iter().find(|s| s.tracking_number == legacy).unwrap();
        assert_eq!(row.status, "delivered", "the no-regress merge held");
        for include_delivered in [true, false] {
            let human = store
                .list_shipments(account, include_delivered, policy)
                .unwrap();
            let agent = agent_hits(&server, include_delivered).await;
            let agent = agent.as_array().unwrap();
            assert_eq!((human.len(), agent.len()), (1, 1), "{include_delivered}");
            assert_eq!(human[0].tracking_number, coming);
            assert_eq!(agent[0]["tracking_number"], coming, "{include_delivered}");
            assert_eq!(agent[0]["status"], human[0].status);
            assert_eq!(agent[0]["status"], "shipped");
            assert_eq!(human[0].legs[0].status, "delivered");
            assert_eq!(agent[0]["legs"][0]["tracking_number"], legacy);
            assert_eq!(agent[0]["legs"][0]["status"], "delivered");
        }
    }

    /// CLEARING A GROUPED CARD CLEARS THE PURCHASE on both doors: the client
    /// posts one id, and no package of that order comes back on either.
    #[tokio::test]
    async fn clearing_a_grouped_card_empties_both_doors() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let policy = ShipmentListPolicy::default();
        let server = SquelchServer::new(store.clone(), "me@localhost")
            .unwrap()
            .with_shipment_policy(policy);
        let hours = |n| Utc::now() - chrono::Duration::hours(n);
        deliver(
            &store,
            account,
            "a",
            hours(3),
            false,
            "1ZW061R3DG21045729",
            "shipped",
            Some("Bean Co"),
            &["5"],
        );
        deliver(
            &store,
            account,
            "b",
            hours(2),
            false,
            "1ZB8B2560323528551",
            "shipped",
            Some("Bean Co"),
            &["5"],
        );
        let card = store
            .list_shipments(account, true, policy)
            .unwrap()
            .remove(0);
        assert_eq!(card.legs.len(), 1);
        assert_eq!(agent_hits(&server, true).await.as_array().unwrap().len(), 1);
        assert!(
            store
                .clear_shipment(account, card.id, Utc::now(), policy)
                .unwrap()
        );
        assert!(
            store
                .list_shipments(account, true, policy)
                .unwrap()
                .is_empty()
        );
        assert!(
            agent_hits(&server, true)
                .await
                .as_array()
                .unwrap()
                .is_empty(),
            "no leg of the cleared card is left on the agent door"
        );
    }

    /// A RESTRICTED MAIL'S MERCHANT AND ORDER NUMBER NEVER REACH THE AGENT. A
    /// pharmacy order mail (restricted) names the store and the order; a plain
    /// carrier notice (allowed) about the same number is what the agent may
    /// see. The package is served, without the pharmacy or the order, and the
    /// hidden order cannot merge it with another box either. The human door
    /// still shows all of it.
    #[tokio::test]
    async fn a_restricted_mails_merchant_and_ref_stay_off_the_agent_door() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let policy = ShipmentListPolicy::default();
        let server = SquelchServer::new(store.clone(), "me@localhost")
            .unwrap()
            .with_shipment_policy(policy);
        let hours = |n| Utc::now() - chrono::Duration::hours(n);
        let number = "1ZW061R3DG21045729";
        deliver(
            &store,
            account,
            "rx",
            hours(3),
            true,
            number,
            "shipped",
            Some("X Pharmacy"),
            &["RX-99"],
        );
        deliver(
            &store,
            account,
            "ups",
            hours(2),
            false,
            number,
            "out_for_delivery",
            None,
            &[],
        );
        let agent = agent_hits(&server, true).await;
        let agent = agent.as_array().unwrap();
        assert_eq!(agent.len(), 1, "the package itself is servable");
        assert_eq!(agent[0]["status"], "out_for_delivery");
        assert!(agent[0]["merchant"].is_null(), "{}", agent[0]);
        assert!(
            agent[0]["orders"].as_array().unwrap().is_empty(),
            "{}",
            agent[0]
        );
        assert!(!agent[0].to_string().contains("Pharmacy"));
        assert!(!agent[0].to_string().contains("RX"));
        let human = store.list_shipments(account, true, policy).unwrap();
        assert_eq!(human[0].merchant.as_deref(), Some("X Pharmacy"));
        assert_eq!(human[0].orders.len(), 1);

        // A second box whose own allowed mail names the same order groups with
        // it for the human, and not for the agent: the link that would merge
        // them came from the restricted mail.
        deliver(
            &store,
            account,
            "box2",
            hours(1),
            false,
            "1ZB8B2560323528551",
            "shipped",
            Some("X Pharmacy"),
            &["RX-99"],
        );
        assert_eq!(
            store.list_shipments(account, true, policy).unwrap().len(),
            1
        );
        let agent = agent_hits(&server, true).await;
        let agent = agent.as_array().unwrap();
        assert_eq!(agent.len(), 2, "no grouping on a link the agent cannot see");
        assert!(
            agent
                .iter()
                .all(|h| h["legs"].as_array().unwrap().is_empty())
        );
        let first = agent
            .iter()
            .find(|h| h["tracking_number"] == number)
            .unwrap();
        assert!(first["merchant"].is_null() && first["orders"].as_array().unwrap().is_empty());

        // One box, the seller named by restricted mail and the order number by
        // allowed mail: the link files the ref under the restricted seller, so
        // it is withheld too.
        let third = "1Z999AA10123456784";
        deliver(
            &store,
            account,
            "rx3",
            hours(1),
            true,
            third,
            "shipped",
            Some("Y Pharmacy"),
            &[],
        );
        deliver(
            &store,
            account,
            "ups3",
            Utc::now(),
            false,
            third,
            "shipped",
            None,
            &["RX-7"],
        );
        let agent = agent_hits(&server, true).await;
        let hit = agent
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["tracking_number"] == third)
            .unwrap()
            .clone();
        assert!(!hit.to_string().contains("Pharmacy"), "{hit}");
        assert!(hit["orders"].as_array().unwrap().is_empty(), "{hit}");
        let human = store.list_shipments(account, true, policy).unwrap();
        let row = human.iter().find(|c| c.tracking_number == third).unwrap();
        assert_eq!(row.orders[0].merchant.as_deref(), Some("Y Pharmacy"));
    }

    /// TWO DOORS, ONE PACKAGE LIST. The agent door applies the human door's
    /// silence rule to its merged list, on both of its paths (a record hit and
    /// a carrier row no record represents), and the same carrier evidence keeps
    /// a row on both. Public API only, so the states are the ones production
    /// can reach.
    #[tokio::test]
    async fn get_shipments_hides_and_keeps_exactly_what_the_human_door_does() {
        use squelch_core::store::agent_triage::AgentCommitOutcome;
        use squelch_core::sync::ingest::{RawFetched, ingest_with_rules};
        use squelch_core::triage::decision::MessageDecision;
        use squelch_core::triage::{CarrierTrack, ShipmentStatus};
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let policy = ShipmentListPolicy::default();
        assert!(policy.stale_after_days > 0 && policy.retired_at_failures > 1);
        let server = SquelchServer::new(store.clone(), "me@localhost")
            .unwrap()
            .with_shipment_policy(policy);

        // Shipping mail that arrived a month ago, so the record's received_at
        // and the carrier row's last_update are both already silent.
        let long_ago = Utc::now() - chrono::Duration::days(30);
        let raw = RawFetched {
            account_id: account,
            gmail_msg_id: "pkg-1".into(),
            gmail_thread_id: Some("pkg".into()),
            raw: format!(
                "From: shop@example.com\r\nTo: me@localhost\r\nSubject: Shipped\r\nDate: {}\r\n\r\nShipped",
                long_ago.to_rfc2822()
            )
            .into_bytes(),
            internal_date: Some(long_ago),
            is_sent: false,
            is_spam: false,
            account_addr: "me@localhost".into(),
        };
        let message = ingest_with_rules(&raw, &Default::default(), long_ago, &[], |_| false);
        let id = store.ingest_message(&message).unwrap();
        let job = store
            .claim_agent_job(account, "triage", Utc::now(), 60)
            .unwrap()
            .unwrap();
        let context = store.load_agent_context(&job).unwrap();
        let decision = MessageDecision {
            summary: "Lamp shipped".into(),
            // Record proposals own record membership independently of destinations.
            records: vec![RecordProposal::Delivery {
                carrier: Some("ups".into()),
                tracking_number: Some("1Z999AA10123456784".into()),
                status: "shipped".into(),
                item_name: None,
                merchant: None,
                order_refs: vec![],
                evidence: vec![],
            }],
            ..Default::default()
        };
        assert_eq!(
            store
                .commit_agent_decision(
                    &job,
                    &context,
                    &decision,
                    std::slice::from_ref(&context.message.source)
                )
                .unwrap(),
            AgentCommitOutcome::Applied
        );
        let row = store.external_shipments(account, true).unwrap().remove(0);
        assert!(row.last_update < Utc::now() - chrono::Duration::days(29));

        let both_doors = || async {
            let agent = values(
                &server
                    .get_shipments(Parameters(GetShipmentsParams {
                        include_delivered: Some(true),
                    }))
                    .await
                    .unwrap(),
            )
            .as_array()
            .unwrap()
            .len();
            let human = store.list_shipments(account, true, policy).unwrap().len();
            (human, agent)
        };
        let in_transit = CarrierTrack {
            status: Some(ShipmentStatus::Shipped),
            carrier_status_raw: "In Transit".into(),
            eta: None,
            delivered_at: None,
        };

        assert_eq!(both_doors().await, (0, 0), "silent on both doors");

        // A carrier answer from back then, never refreshed, vouches for nothing.
        store
            .apply_carrier_track(account, row.id, &in_transit, long_ago)
            .unwrap();
        assert_eq!(
            both_doors().await,
            (0, 0),
            "a stale answer keeps it on neither"
        );

        // The same answer today is a no-change poll: last_update stays a month
        // old and the vouching alone keeps the row, on both doors.
        store
            .apply_carrier_track(account, row.id, &in_transit, Utc::now())
            .unwrap();
        assert_eq!(both_doors().await, (1, 1), "vouched on both doors");
        let as_record = values(
            &server
                .get_shipments(Parameters(GetShipmentsParams {
                    include_delivered: Some(true),
                }))
                .await
                .unwrap(),
        );
        assert_eq!(
            as_record[0]["item_name"], "Lamp shipped",
            "served as a RECORD HIT (the decision summary) on this path"
        );

        // Rejected into retirement, the carrier no longer vouches.
        for _ in 0..policy.retired_at_failures {
            store
                .record_poll_outcome(account, row.id, Utc::now(), true)
                .unwrap();
        }
        assert_eq!(both_doors().await, (0, 0), "retired on both doors");

        // MAIL RECONCILE REJECTED IS NOT NEWS ON EITHER DOOR. A newer mail the
        // model read as a delivery but without a carrier never reaches the row
        // (reconcile drops it), so the human door still sees a silent row. The
        // agent door lists that mail as a record hit and must age it by the
        // row's clock, not the mail's, or exactly the hidden rows disagree.
        let fresh = seed(&store, account, "pkg", "Where is my package");
        let job = store
            .claim_agent_job(account, "triage", Utc::now(), 60)
            .unwrap()
            .unwrap();
        let context = store.load_agent_context(&job).unwrap();
        let carrierless = MessageDecision {
            summary: "Delivery question".into(),
            records: vec![RecordProposal::Delivery {
                carrier: None,
                tracking_number: Some("1Z999AA10123456784".into()),
                status: "unknown".into(),
                item_name: None,
                merchant: None,
                order_refs: vec![],
                evidence: vec![],
            }],
            ..Default::default()
        };
        assert_eq!(
            store
                .commit_agent_decision(
                    &job,
                    &context,
                    &carrierless,
                    std::slice::from_ref(&context.message.source)
                )
                .unwrap(),
            AgentCommitOutcome::Applied
        );
        assert_eq!(
            both_doors().await,
            (0, 0),
            "a carrierless mail today revives the package on neither door"
        );
        store
            .set_attention_status(account, fresh, squelch_core::types::AttentionStatus::Done)
            .unwrap();

        // THE OTHER PATH of the agent door. A record the user marked done leaves
        // the agent feed, and its carrier row is then served by the loop over
        // rows no record represents. That loop must judge by the same rule.
        store
            .apply_carrier_track(account, row.id, &in_transit, Utc::now())
            .unwrap();
        store
            .set_attention_status(account, id, squelch_core::types::AttentionStatus::Done)
            .unwrap();
        let agent_only = values(
            &server
                .get_shipments(Parameters(GetShipmentsParams {
                    include_delivered: Some(true),
                }))
                .await
                .unwrap(),
        );
        assert_eq!(
            agent_only[0]["item_name"], row.item_name,
            "served by the carrier-row loop (the row's own name), not as a record hit (the summary)"
        );
        // apply_carrier_track zeroed the failures without a visible change, so
        // last_update is still a month old and only the vouching lists it.
        assert_eq!(
            both_doors().await,
            (1, 1),
            "unrepresented row, vouched, on both"
        );
        for _ in 0..policy.retired_at_failures {
            store
                .record_poll_outcome(account, row.id, Utc::now(), true)
                .unwrap();
        }
        assert_eq!(
            both_doors().await,
            (0, 0),
            "unrepresented row, unvouched, on neither"
        );
    }

    fn values(result: &CallToolResult) -> serde_json::Value {
        serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
    }

    async fn search(server: &SquelchServer, query: &str) -> serde_json::Value {
        values(
            &server
                .search_mail(Parameters(SearchMailParams {
                    query: query.into(),
                    k: Some(50),
                    sort: None,
                }))
                .await
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn pending_and_restricted_mail_fail_closed_but_human_can_read() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let server = SquelchServer::new(store.clone(), "me@localhost").unwrap();
        let restricted = seed(&store, account, "auth", "password reset");
        decide(&store, account, restricted, true, None);
        let pending = seed(&store, account, "pending", "unassessed message");
        for id in [
            restricted.to_string(),
            pending.to_string(),
            "unknown".into(),
        ] {
            let error = server
                .get_thread(Parameters(GetThreadParams { id }))
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                ErrorData::resource_not_found("not found", None).code
            );
        }
        assert!(
            search(&server, "password")
                .await
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            search(&server, "unassessed")
                .await
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(store.thread_view_with_html(account, "auth").is_ok());
        assert!(store.thread_view_with_html(account, "pending").is_ok());
    }

    #[tokio::test]
    async fn informational_login_alert_is_readable_after_allowed_assessment() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let id = seed(&store, account, "login", "new login alert");
        decide(&store, account, id, false, None);
        let server = SquelchServer::new(store.clone(), "me@localhost").unwrap();
        assert_eq!(search(&server, "login").await.as_array().unwrap().len(), 1);
        let view = server
            .get_thread(Parameters(GetThreadParams { id: id.to_string() }))
            .await
            .unwrap();
        assert_eq!(values(&view)["thread_id"], "login");
    }

    #[tokio::test]
    async fn restricted_sibling_blocks_full_thread_and_search_projection() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let first = seed(&store, account, "mixed", "ordinary conversation");
        decide(&store, account, first, false, None);
        let secret = seed(&store, account, "mixed", "reset token");
        decide(&store, account, secret, true, None);
        let server = SquelchServer::new(store.clone(), "me@localhost").unwrap();
        assert!(
            server
                .get_thread(Parameters(GetThreadParams {
                    id: first.to_string()
                }))
                .await
                .is_err()
        );
        assert!(
            search(&server, "ordinary")
                .await
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn derived_summary_cannot_disclose_a_restricted_source() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let secret = seed(&store, account, "secret", "reset code");
        decide(&store, account, secret, true, None);
        let derived = seed(&store, account, "derived", "public update");
        decide(&store, account, derived, false, Some(secret));
        let server = SquelchServer::new(store.clone(), "me@localhost").unwrap();
        assert!(!store.external_thread_allowed(account, "derived").unwrap());
        assert!(
            search(&server, "public")
                .await
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn inbox_uses_agent_membership_and_does_not_acknowledge_human_reading() {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let account = store.ensure_account("me@localhost").unwrap();
        let id = seed(&store, account, "attention", "please reply");
        decide(&store, account, id, false, None);
        let server = SquelchServer::new(store.clone(), "me@localhost").unwrap();
        let result = server
            .get_inbox_updates(Parameters(GetInboxUpdatesParams {
                since: Utc::now() - chrono::Duration::days(1),
                min_importance: Some(100),
            }))
            .await
            .unwrap();
        let output = values(&result);
        assert_eq!(output.as_array().unwrap().len(), 1);
        assert_eq!(output[0]["message_id"], id);
        assert!(output[0].get("tier").is_none());
        assert!(output[0].get("notification").is_none());
        assert!(
            store
                .agent_read_message(account, id)
                .unwrap()
                .opened_at
                .is_none()
        );
    }
}
