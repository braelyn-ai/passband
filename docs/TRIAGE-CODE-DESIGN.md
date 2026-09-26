# Agentic triage: code design

Status: implementation design based on the agreed [product plan](TRIAGE-REWRITE.md). The implemented module layout consolidates validation in `triage/agent.rs`, evidence execution in `sync/triage_worker.rs`, and durable storage in `store/sqlite/agent_triage.rs`. Records live in typed decision JSON. See [TRIAGE-OPERATIONS.md](TRIAGE-OPERATIONS.md) for shipped behavior, configuration, and limitations. Numeric tuning defaults remain starting hypotheses for evaluation.

## 1. Architecture

One lead owns domain types, storage contracts, orchestration, and integration. Build a complete minimal path before parallel implementation against those contracts.

```text
Gmail / RFC822
      |
parse, sanitize, persist message + durable jobs
      |
      +---- notification worker ---- small model ---- notification request ---+
      |                                                                      |
      +---- triage worker ---- context + bounded agent loop                    |
                                   |                                         |
                              validated decision                             |
                                   |                                         |
                    atomic commit + optional notification request -----------+
                                   |                                         |
                 placements, records, attention, access             delivery ledger
                                   |                                         |
                           API / ordered FYE                         existing push transport

notification tap ---- human message read (available before triage completes)
```

The model decides meaning and placement. Code validates contracts, executes account-scoped reads, persists state, ranks already-selected FYE items, enforces external access, and coordinates delivery. There is no heuristic seed or score-derived placement.

## 2. Module boundaries

Use concrete modules and existing infrastructure. Do not introduce a generic agent/workflow framework.

| Module | Responsibility |
| --- | --- |
| `sync/ingest.rs` | MIME parsing, body selection, sanitization, transport identity and metadata; produces `ParsedMessage` |
| `triage/decision.rs` | Typed model output, evidence references, category/destination definitions |
| `triage/context.rs` | Thread context, explicit preferences, corrections, version snapshot, reserved memory provider |
| `triage/tools.rs` | Small, closed set of account-scoped evidence retrieval operations |
| `triage/agent.rs` | Bounded loop: ask model, execute requested reads, validate final decision |
| `triage/prompts/` | Versioned plain-text agent and notification prompts plus category/ranking anchors |
| `triage/validation.rs` | Structural validation and reference ownership; never semantic score floors or genre clamps |
| `triage/ranking.rs` | Pure ranking function and named score contributions |
| `triage/access.rs` | Explicit external-access checks and derivative provenance checks |
| `triage/records.rs` | Typed record proposals and conversion to existing specialist store types |
| `triage/llm.rs` | Existing provider transport, response validation, usage accounting, redacted errors |
| `sync/triage_worker.rs` | Durable job claiming, budgets, retries, commit and stale-result handling |
| `sync/notify_lane.rs` | Independent small-model notification job; no seed, tier, or category dependency |
| `triage/notify_llm.rs` | Notification-only model contract, including auth classification |
| `triage/notifications.rs` | Eligibility and notification-request contract shared by both paths |
| `store/sqlite/triage_jobs.rs` | Leases and retry state |
| `store/sqlite/triage_decisions.rs` | Atomic decision commit and current projections |
| `store/sqlite/attention.rs` | FYE candidates and explicit user state; no semantic membership inference |
| `store/sqlite/notify.rs`, `events.rs` | Decision attempts, logical-event deduplication, durable delivery requests |

Keep domain types outside the already-large `store/mod.rs`; re-export only where necessary. The existing synchronous `Store` trait and SQLite delegation pattern can remain. Never hold a SQLite lock across a model call.

## 3. Domain contracts

Illustrative Rust declarations specify the shape, not copy-ready implementation. ID wrappers, timestamps, and bounded numeric types use existing crate conventions where available.

```rust
enum EmailKind {
    Correspondence, Editorial, Promotional, Bill, Receipt,
    FinancialUpdate, Delivery, EventReservation, AccountService,
    AuthenticationSecurity, General,
}

enum Destination { Fye, Reading, Records }
enum MessageDestination { Reading }
enum ExternalAccess { Pending, Allowed, Restricted }
enum AttentionState { NeedsUser, WaitingOnOthers, Informational, Resolved }
enum ActionKind { Reply, Pay, Decide, Attend, Review, Other }

struct MessageDecision {
    kinds: Vec<EmailKind>,                 // nonempty, unique; mixed content allowed
    destinations: Vec<MessageDestination>, // unique; FYE belongs to thread attention
    summary: String,
    reason: String,
    auth: AuthAssessment,
    external_access: AccessAssessment,     // final: allowed or restricted, never pending
    attention: ThreadAttentionDecision,
    records: Vec<RecordProposal>,
    related_updates: Vec<RelatedAttentionUpdate>,
    rule_exceptions: Vec<RuleException>,
    notification: NotificationAdvice,     // auth comes from this decision's canonical auth field
    revisit: Option<RevisitRequest>,
}

struct AuthAssessment {
    kinds: Vec<AuthKind>, // OTP, reset, sign-in link, verification, login alert, security alert
    evidence: Vec<EvidenceRef>,
}

struct AccessAssessment {
    restricted: bool,
    reason: String,
    evidence: Vec<EvidenceRef>,
}

struct ThreadAttentionDecision {
    show_in_fye: bool,
    state: AttentionState,
    actions: Vec<AttentionAction>,
    summary: String,
    factors: AttentionFactors,
    relevant_message_ids: Vec<MessageId>,
    evidence: Vec<EvidenceRef>,
}

struct AttentionFactors {
    urgency: UnitScore,
    action_need: UnitScore,
    personal_relevance: UnitScore,
    importance: UnitScore,
    attention_at: Option<SupportedTime>,
}

struct NotificationAssessment {
    auth: AuthAssessment,
    advice: NotificationAdvice,
}

struct NotificationAdvice {
    importance: Score100,
    title: String,
    body: String,
    reason: String,
}

struct RuleException {
    rule_id: RuleId,
    reason: String,
    evidence: Vec<EvidenceRef>,
}
```

Auth classification and access restriction are independent: a login alert is auth, pushes, and is normally externally allowed; a reset link is auth, pushes, and is restricted. Generic account-settings links do not count as credentials. The model makes that distinction. Validation requires a reason/evidence for restriction but does not reproduce a detector.

`SupportedTime` preserves date-only versus timestamp precision, stated timezone when available, source message, and whether a relative time was interpreted. Unknown deadlines stay unknown; no fabricated fallback dates. `EvidenceRef` identifies an account-owned message/attachment and a bounded location or field. Do not persist credentials in quoted evidence or diagnostic reasons.

The executor supplies message/account IDs, run metadata, model/config versions, and expected revisions. The model cannot select an account, grant itself access, or manufacture store revisions.

### Message placement and thread attention

Categories, Reading placement, and typed record facts are message-level facts. FYE membership belongs only to `thread_attention.show_in_fye`; do not persist a competing FYE flag on old message rows. FYE presents one current attention item per provider thread, with multiple unresolved actions inside it. API projections combine these scopes so a receipt may appear in Records and belong to an active FYE item. A singleton uses the existing fallback thread identity. The thread decision explicitly accounts for active messages and prior actions; a new low-value message does not automatically erase an earlier unresolved request.

The agent may fetch another thread and propose a narrowly scoped update to resolve a related obligation, with evidence IDs and the target attention revision. This supports a receipt resolving a bill sent in a different thread. No general-purpose situation graph is required in this rewrite. Cross-thread merging and arbitrary entity resolution are deferred; do not approximate them with merchant/amount heuristics.

Actions have durable IDs, source revisions, and inferred state separate from explicit completion. New action proposals receive executor-assigned IDs; subsequent decisions reference fetched existing IDs. Explicit user `done`, snooze, reminder, and correction state is stored separately. A sender preference may be overridden with a recorded reason; a direct action on the current item is preserved. New incoming mail creates a new thread content revision, allowing an explicitly identified new action to surface without reviving an old completed action. An unrelated new sibling is not permission to reopen everything. Related-thread resolution updates both inferred action state and FYE membership atomically.

## 4. Agent loop and evidence tools

Start with one capable model and optional configured stronger review model. The first call can finish without tools. A review request names the question it needs resolved, uses the remaining run budget, and cannot recursively escalate. Do not implement a deterministic escalation router.

Use a typed `AgentStep` response: either `ReadEvidence { requests }`, `RequestReview { question }`, or `Finish { decision }`. This can use the existing structured-output transport initially; native provider tool calling is not required for the first implementation. Prior steps and tool results are explicit bounded conversation/context entries. Keep the execution interface independent of the provider wire format.

Initial context includes:

- Current message content, actual sender/recipient and transport timestamps, truncation indicators, and attachment inventory.
- Recent thread messages and existing unresolved attention state, with fetch handles for omitted content.
- Matching sender preferences, structured corrections, and observed sender interactions. Contact history is evidence, never a score floor.
- Prior decision, run trigger, account timezone, and an empty learned-memory slice.

Initial tool set:

| Tool | Returns |
| --- | --- |
| `read_thread` | Account-owned thread messages, prior attention state, explicit user state |
| `read_message` | Full or paged message text and mechanically parsed link metadata |
| `search_mail` | Bounded account-scoped candidates, with IDs for follow-up reads |
| `read_sender_history` | Recent messages, interaction facts, preferences and corrections |
| `read_record` | Existing receipt/bill/shipment/calendar/banking facts and source IDs |
| `read_attachment` | Bounded text for supported formats, or explicit unsupported/unavailable result |

Internal tools may read restricted mail; public MCP tools must never be reused as the internal context layer because their access rules differ. Do not fetch email URLs or execute attachment content. Parsing attachment text is mechanical; absent PDF/image support is explicit rather than silently treated as an empty attachment. Implementation must choose a supported reader or report that limitation in the decision, with human-readable pending/review state when required evidence cannot be obtained.

Tools only read. The final decision proposes writes, which one transaction validates and applies. All related updates must refer to evidence and state revisions the run actually fetched. The executor also records every content source exposed in initial context and tool results, including search snippets. These consumed sources conservatively govern derivative access; model-selected citations explain decisions but cannot omit a restricted source to authorize external disclosure. Invalid output gets a bounded repair attempt, then an explicit failed/retry state; never a fabricated classification.

### Future memory seam

```rust
trait MemoryContextProvider {
    fn relevant_memory(&self, scope: &ContextScope) -> Result<Vec<MemoryEntry>>;
}
```

Ship `EmptyMemoryContext`. Reserve a separate future `MemoryEditor` capability boundary in documentation; do not register an active write tool, create a memory table, or expose memory UI. Future entries need ID, scope, provenance, revision, and deletion support. Email text and inferred memory must not acquire the authority of direct user instructions. Issue [#213](https://github.com/braelyn-ai/passband/issues/213) owns preference-feedback UX.

## 5. Persistence and concurrency

Use additive migrations and a one-time cutover marker, following the repository's idempotent schema installation style. New authoritative tables replace stage sentinels; legacy rows remain only for migration/rollback until cleanup.

| Table/projection | Key and contents |
| --- | --- |
| `triage_jobs` | Account, message/thread target, job kind, trigger, input revision, state, lease token/expiry, attempts, available-at, redacted last error |
| `triage_runs` | Run ID, target revision, trigger, model/prompt/schema/config versions, timing, token usage, status; no raw model transcript |
| `message_decisions` | Current message decision, revision, run ID, categories, summary, auth/access assessment; bounded structured JSON plus indexed access fields |
| `message_destinations` | Unique account/message rows for Reading; specialist record groups derive from typed facts |
| `thread_attention` | Unique account/thread, revision, show-in-FYE, current state/actions/factors/summary, relevant message IDs, waiting-since, last relevant activity |
| `attention_user_state` | Account/thread, explicit done-through content revision, completed action IDs, snooze/reminder, user-state revision |
| `message_read_state` | Account/message, opened-at and acknowledged content revision; no inference from prefetch or opening another sibling |
| `triage_corrections` | Account/target/field, explicit value, source content revision and correction revision; preserves field-scoped user intent |
| `decision_sources` | Derived decision/record/attention ID to source message IDs and source revisions |
| existing specialist tables | Typed records, with decision/run provenance and stale/retired state |
| existing notify/event tables | Extend for independent assessments and unique logical notification key |

Add a message content revision and a thread content revision. Capture preference and user-state revisions in each run. IDs used across tables remain account-qualified even when local integer IDs are globally unique today.

Ingest transaction: insert/update the parsed message, advance revisions only for meaningful content changes, mark newly changed content access-pending, and enqueue triage plus eligible notification jobs. Duplicate fetches create neither duplicate jobs nor new arrival eligibility. Sent messages are context and trigger affected thread re-evaluation; they do not receive arrival pushes or FYE cards of their own. Preserve provider spam as source metadata; regular inbox triage does not silently pull spam into FYE. Explicit spam-review requests can enqueue it separately without historical pushes.

Sent and spam mail still require model access assessment before any external exposure. Schedule access-review jobs owned by the triage worker for messages excluded from placement processing; these use a narrow structured access contract and cannot generate placement or arrival pushes. Do not carry forward the old sent/spam bypass as implicit access permission.

Worker lifecycle: `queued -> leased -> completed`, with `retry_at` or `failed` on errors. Expired leases can be reclaimed; each commit checks the lease token and expected revisions. Durably queued work survives restart. Fresh arrivals have scheduling priority over migration, with bounded fairness so old work progresses.

Commit validates current message/thread/preferences/user-state revisions and every related target revision. A conflict discards the proposed mutations and queues a fresh run, recording its already-spent usage. Apply message decisions, placements, thread attention, related resolution, record changes, access state, revisit jobs, and a requested notification event in one transaction. No partial cross-thread bill closure. Never let stale leased workers overwrite a newer decision.

For ordinary revisits, the previous valid placement remains visible until replaced. Initial arrival has no provisional placement. Content changes invalidate external access until reassessed; mere scheduling of a revisit need not revoke a valid unchanged message assessment. User corrections win immediately in their applicable scope and invalidate conflicting in-flight work.

Store contract sketch:

```rust
fn ingest_pending(message: ParsedMessage, origin: IngestOrigin) -> Result<Ingested>;
fn claim_triage_job(now: DateTime<Utc>, lease: Duration) -> Result<Option<ClaimedJob>>;
fn load_triage_context(job: &ClaimedJob) -> Result<ContextSnapshot>;
fn commit_triage(run: ValidatedRun) -> Result<CommitOutcome>; // Applied | Stale
fn request_notification(request: NotificationRequest) -> Result<NotifyOutcome>;
fn acknowledge_message(target: MessageRevision) -> Result<()>;
fn apply_user_action(action: VersionedUserAction) -> Result<()>;
```

`commit_triage` calls the same notification arbitration helper on its existing transaction, not a second nested transaction. Logical event delivery stays asynchronous. A fired explicit user reminder surfaces the target until handled even if the model's FYE flag is false; this is direct user instruction, not semantic classification. Category corrections do not freeze action resolution. Explicit access restriction corrections immediately invalidate dependent external projections.

## 6. Notifications

The small-model contract contains auth classification, notification importance, safe push text, and a reason. It has no tier, inferred bill detector result, or placement. Pass strong sender preferences as context; do not bypass the model for sender rules. All identified auth qualifies regardless of score or sender preference; other mail qualifies at `notification.importance_threshold`.

The full agent returns its own notification assessment and may request delivery after a fast decline, timeout, or missed auth classification. Qualification is independent of FYE membership and triage importance. A notification tap reads by stable message ID through the human endpoint and does not wait for an access or placement decision.

Preserve the existing first-observed arrival eligibility concept and configurable freshness/rescue windows. Stamp eligibility once from ingest origin and transport receipt facts, not re-triage time. Backfill, migration, and sent copies have no arrival notification eligibility. Proposed initial timing uses the existing 15-minute first-arrival and 60-minute late-delivery windows; these are tuning defaults, not auth-score exceptions. An expired code is not made fresh by retrying a model call.

Both paths submit the same arrival notification key `(account_id, message_id, arrival)`. Reminders use a distinct reminder occurrence ID. A later genuinely new development uses its actual new message or explicit event identity, never a random key to bypass deduplication. Store all assessments separately from the single logical delivery event.

Arbitrate in one transaction: check arrival eligibility, existing event, global controls, and explicit opened/handled/snoozed state, then enqueue or record the reason for withholding. Proposed default: do not deliver a delayed arrival push after the user has already opened or handled that message. This is a lifecycle acknowledgement, not an importance-based auth suppression. Recheck before an unsent event is dispatched where possible; a push already handed to APNs cannot be recalled.

Do not claim exactly-once push transport. Ensure one logical event in the database; reuse stable event IDs and existing collapse/dedup facilities through relay/client delivery. A crash after provider acceptance but before local acknowledgement can still cause transport replay. Tests must cover that boundary honestly.

Existing sealed kind-only notification content is a useful conservative default for actionable auth. Login alerts can have safe descriptive text. Classification is by models, not content-redaction regexes that make semantic decisions. Keep internal notification assessments out of agent-facing feeds.

## 7. FYE ranking

Membership comes only from current model placement plus explicit user state. Query all eligible candidates within the display window, including unresolved attention outside that window. Rank in one Rust function used by every consumer. Do not limit candidates by old importance order before applying the new rank.

Proposed starting formula, all components bounded to `[0, 1]`:

```text
score = 30 * urgency
      + 25 * action_need
      + 20 * personal_relevance
      + 20 * recency
      +  5 * waiting
      +  2 * importance
      - 15 * ai_generated * (1 - max(action_need, personal_relevance))

recency = 2 ^ (-hours_since_relevant_activity / recency_half_life_hours)
waiting = min(unresolved_wait_days / waiting_saturation_days, 1)
```

Starting `recency_half_life_hours = 24`, `waiting_saturation_days = 7`. Waiting contribution applies to unresolved user actions, and its clock survives re-triage. Relevance/activity timestamps derive from agent-selected source messages, validated against real message timestamps; a background run cannot manufacture recency. Urgency is assessed by the agent with an optional supported attention time and scheduled re-evaluation; avoid secretly converting category labels into urgency weights.

`RankBreakdown` includes raw values, weighted contributions, total, config version, and evaluation time. Return it in human diagnostics; the ordinary UI needs only the useful explanation. Tie-break by relevant activity descending then stable item ID. Pagination freezes ranking time/config and candidate revisions in a short-lived snapshot so changing recency does not duplicate or skip rows between pages. Refresh creates a new snapshot. No semantic ranking SQL scattered across endpoints.

The numeric defaults must be tested on actual examples before shipping. `ai_generated` is the model's likelihood that the text was machine-written, informed by stylometric hints (`triage/ai_text.rs`) passed in its context. It is a penalty only, never a membership rule: people send AI-drafted mail too, so the penalty vanishes as action need or personal relevance approach 1, and the prompt forbids hiding a thread from FYE on authorship alone. Importance's maximum contribution is deliberately small; configuring it to dominate should require an explicit documented policy change rather than happen accidentally.

## 8. Records and external access

The agent returns typed record proposals or requests model extraction when needed, charged to its run budget. Preserve existing receipt, shipment, banking, and calendar presentation/store machinery where it can accept explicit facts. A record is not a reason to suppress FYE. Model-produced records replace speculative ingest extraction and category-clamp behavior.

Retain mechanical timestamp/currency parsing, validation, MIME/calendar decoding, exact identifier storage, and carrier API polling. Remove heuristic record detection, inferred shipping triggers, and merchant/amount-based bill closure. Retrieval may offer candidate related bills; only an evidence-backed model update resolves one. Re-triage reconciles records from that decision without deleting independent carrier updates or human edits.

Carrier observations retain their own timestamp/provenance and can enqueue agent re-evaluation; they do not manufacture a new mail-arrival event. Partial payments and ambiguous matches may leave an obligation unresolved. Reversals can reopen an inferred resolution, while explicit user completion stays separately recorded. Remove blanket `done`/human-corrected exclusions from model revisit queues; evaluate only the fields the model is allowed to update.

Three read audiences are explicit: human, internal triage, and external agent. Human and internal readers can access pending/restricted mail. External reads require current `Allowed` assessments. Missing/null/unknown access values are pending, never normal. Pending is not sealed and must not trigger auth shredding or retention deletion.

Track executor-recorded consumed sources for summaries, records, and attention items. A derivative is externally eligible only while every source it used has an allowed current access assessment. Initially retain whole-thread exclusion for external thread reads containing pending/restricted messages; filtered mixed-thread summaries require a separately designed regeneration path. Human thread/message reads do not share this exclusion. Plain message-level search may return allowed messages, but contextual snippets and thread summaries must obey source checks.

Existing search/embedding code must not expose old indexed content merely because an index contains it. Apply access checks at query time for both keyword and vector results and every record endpoint. Existing internal indexes can remain; storing vectors is not granting external access. Reclassification invalidates dependent external projections atomically.

## 9. API and client contracts

Introduce a versioned human API and explicitly advertise its version/capabilities. Proposed endpoints:

| Endpoint | Contract |
| --- | --- |
| `GET /client/v2/feed?destination=fye` | Server-ranked `AttentionItem` page and ranking snapshot cursor |
| `GET /client/v2/feed?destination=reading` | Message-based Reading items with real receipt timestamps; grouping is presentation only |
| `GET /client/v2/feed?destination=records` | Aggregate of typed Calendar, Shipping, Billing, and Receipt facts; no generic fallback |
| `GET /client/v2/messages/{message_id}` | Human message plus thread context/focus target; pending/restricted supported |
| `POST /client/v2/messages/{message_id}/opened` | Exact displayed-message acknowledgement, independent of done state |
| `POST /client/v2/attention/{item_id}/actions` | Versioned done/snooze/reminder and undo operations |
| `POST /client/v2/messages/{message_id}/corrections` | Field-scoped kind/placement/access corrections; thread attention corrections explicitly identify the thread/item |
| `GET /client/v2/triage/{message_id}` | Processing state, current decision, reasons, rule exceptions, versions and rank breakdown |

Each request retains the existing human authentication/account boundary. A notification target carries account identity plus stable message ID and optionally thread ID. The client resolves the account, opens that exact message, and records only messages actually displayed. Retain HTML sanitization, attachment controls, and sensitive-content cache handling; remove the old auth-tap routing to a generic Auth page.

`AttentionItem` contains item ID/revision, thread ID, representative message ID, kinds, combined destinations, summary, actions/state, relevant activity timestamp, rank total and explanation. `ReadingItem` contains message ID, thread ID, sender, summary and actual received timestamp. No required legacy `tier` field. All clients preserve server FYE order, including keyboard navigation and prefetched reading queues.

Remove `Lib/Ranking.swift` scoring and the competing `Prefs.rankWeight` slider. Replace `Newsletters.derive` and `NewsletterFeed` noise-tier/marketing/robot/repetition heuristics with the Reading endpoint. Sender-card grouping may remain as a pure presentation step. Rename caches/views/onboarding/accessibility copy consistently. The existing `/client/marketing` extraction endpoint is not simply renamed into Reading: promotion facts and destination membership are different contracts.

Update `AppStore` standing/new/open band state to one FYE list. Adapt `SitrepView`, `MobileSitrepView`, `SitrepPoller`, `Newsletters.swift`, `SitrepZones`, `WireTypes`, `TriageTargets`, `Notifier`, notification extension, and onboarding fixtures. Counts/badges must use a documented server aggregate; `NeedToday` deadline/importance logic cannot independently decide attention membership.

Preserve explicit rule-editor `sweep` as a separate user action on existing mail if offered; saving a preference alone enqueues evaluation rather than directly rewriting tiers. Rule exceptions replace UI wording that implies a rule always governs. Keep record cards, correction palette mechanics, account switching, reader and cache infrastructure with revised inputs.

Audit TUI, MCP, control/CLI, desktop bridge, and debug endpoints for legacy DTO use. Public agent APIs must use access-checked new projections, not translate new placements back through legacy importance thresholds. Human actions on restricted mail must work too; the read endpoint alone is insufficient.

## 10. Configuration

Organize settings by actual responsibility:

```toml
[triage.agent]
# model and optional review_model resolved from deployment configuration
max_model_turns = 4
max_tool_calls = 8
max_review_calls = 1
timeout_secs = 90
concurrency = 2

[triage.context]
initial_thread_messages = 8
max_related_messages = 20
# explicit byte/token and attachment limits selected with the provider adapter

[notify]
min_importance = 50
fast_timeout_secs = 8
fast_concurrency = 4
freshness_window_secs = 900
rescue_window_secs = 3600

[triage.ranking]
urgency_weight = 30
action_need_weight = 25
personal_relevance_weight = 20
recency_weight = 20
waiting_weight = 5
importance_weight = 2
ai_generated_penalty_weight = 15
recency_half_life_hours = 24
waiting_saturation_days = 7

[sync]
backfill_days = 30
```

These are supported configuration fields; see [TRIAGE-OPERATIONS.md](TRIAGE-OPERATIONS.md) for enforced spend and revisit limits. Bound total input/output tokens and account spend as well as wall time; retain existing usage ledgers and gateway/model resolution rather than hardcoding a model or price in this design. Reserve model-turn budget for a final answer; exhaustion without a valid answer produces pending/retry, not an invented result. Keep fast-notification capacity separate from migration/investigation work.

Validate positive durations and budgets, bounded scores, finite nonnegative weights, and meaningful ranking denominators. Document settings with units and tuning examples. Store a hash/version of nonsecret effective configuration with each run. Migrate deployment env names deliberately; reject conflicting old/new settings rather than silently choosing a different behavior. No settings UI in this rewrite.

## 11. Cutover and compatibility

1. Back up the database consistently, including WAL handling, and record daemon/client versions.
2. Install additive schema and migrate explicit user state, preserving provenance. Historical automatic bill closures must not be mistaken for direct user completion; use existing audit evidence where available and mark uncertain provenance for evaluation.
3. Disable old stage/extractor/revisit semantic workers before enabling new workers. New code does not dual-write legacy verdicts.
4. Mark legacy decisions nonauthoritative for new placement and access. Queue recent mail (initial proposal: 30 days), unresolved items, active reminders, and required related context. Prioritize new arrivals; all migration jobs are push-silent.
5. Legacy mail outside that window remains human-readable and pending for external access until classified. Explicit external requests may schedule assessment and return a generic unavailable result; never allow legacy `normal` to grant new access by default. Offer explicit historical re-triage for broader coverage.
6. Deploy the daemon before the matching clients. New clients probe `/client/v2/capabilities` and display an explicit daemon-upgrade requirement if the required contract is missing. Mobile rollout is not atomic; there is no legacy score-derived placement fallback.
7. Remove old implementation after integrated verification. Additive legacy tables may remain temporarily for recovery; they are not queried as fallback classifiers.

Rollback means a coordinated compatible binary plus database backup, with a documented treatment of mail and user actions received after backup. Do not imply that restoring an old binary against a migrated, actively written database is safe. No production shadow/cohort rollout is required; offline comparisons and migration rehearsal remain part of validation.

## 12. Deletion and reuse map

| Existing area | Change |
| --- | --- |
| `triage/mod.rs::stage1_with_config` | Delete semantic rungs, seed verdicts, stage guards |
| `triage/seal.rs::detect_sealed` | Delete regex sealing; replace with model access assessment |
| `triage/router.rs` | Delete detector disagreement, score-boundary and contact escalation rules |
| `triage/stage1_llm.rs`, `stage2.rs` | Replace stage prompts/apply paths; remove known-contact floors, tier derivation, record clamps and post-model sender overrides |
| `sync/mod.rs::{stage1_pass,stage2_pass,extract_pass,revisit_pass,emit_seed_event}` | Replace with small workers; preserve transport/sync cursor behavior |
| `sync/ingest.rs` | Keep parsing/sanitization; remove semantic sealing, classification, receipt/shipment/calendar decisions |
| `triage/deadline.rs`, `receipt.rs`, `shipment.rs`, `calendar.rs` | Separate reusable parsing/data utilities from detectors; delete semantic detectors and inferred due dates |
| `triage/receipt_match.rs`, `store/sqlite/specialists.rs::auto_close_bill_for_receipt_conn` | Remove automatic merchant/amount-based resolution; relocate utilities still used for mechanical identifiers |
| `store/sqlite/messages.rs` receipt/calendar auto-resolution; `specialists.rs::banking_apply` | Remove category/record-driven writes to user `done` state, including on re-ingest |
| `specialists.rs` re-detect/reparse cleanup and shipment visibility rules | Remove regex-authoritative cleanup and tracking-shape/age-based semantic hiding; preserve explicit clears and authoritative carrier observations |
| `triage/extract/*` | Reuse appropriate schemas/parsers/prompt fragments behind agent-owned extraction; remove category-only/regex-triggered queues and sealed-input prohibitions |
| `triage/revisit.rs` | Replace tier/FYE heuristics with agent-requested and explicit user reminder scheduling |
| `store/sqlite/triage_stages.rs` | Replace stage queues/applies; retain usage accounting and useful evidence queries with corrected access audience |
| `store/sqlite/attention.rs` | Remove `STANDING_BAND`, score-threshold membership, `age * importance` ordering |
| `sync/notify_lane.rs`, `triage/events.rs` | Remove `Seed`, `Sealed` bypass, sender-suppression bypass, contact floors and tier-derived event qualification; retain useful freshness/identity mechanics |
| `triage/rule_infer.rs`, sender rule apply paths | Audit and remove automatic preference mutation/retroactive tier rewrites; preserve explicit user-created preferences as agent context |
| `triage/llm.rs`, credentials/gateway transport | Reuse and extend typed structured-step support |
| `carriers/*`, push relay/APNs | Retain transport and authoritative external updates; adapt event/provenance contracts |
| `docs/SECURITY.md`, `NOTIFY.md`, developer docs/metrics | Rewrite around new access boundary, lifecycle, settings and decision versions |

## 13. Worked cases

1. **2FA code:** small model recognizes auth and requests push. Human taps before triage completes and reads it. Agent later marks actionable auth restricted; no FYE score or human access depends on that restriction. One logical arrival event.
2. **Login alert:** auth push; full triage decides whether it needs FYE and grants external access if no credential is present. Auth category alone never seals it.
3. **Sale from a normal sender:** Reading by default; likely below push threshold. A strong sender suppression preference usually makes it quietly filed, with no deterministic override afterward.
4. **Service cancellation from a suppressed sender:** agent identifies actual consequence, records its rule exception, selects FYE and may request a later push after the fast model declined.
5. **Bill followed by receipt:** agent retrieves the outstanding bill and supporting records, proposes a versioned resolution. Receipt remains Records; the resolved obligation leaves active FYE unless other actions remain. Amount/domain similarity alone does not close it.
6. **User marks done while model runs:** stale commit loses its revision check; retry preserves direct user completion. No delayed arrival push after acknowledgement under the proposed lifecycle default.
7. **New ordinary personal message versus old unresolved bill:** both qualify for FYE by model decision. Recency helps the new message; urgency/action need keep a consequential old obligation competitive. Diagnostics explain each contribution.
8. **Provider outage:** parsed mail is human-readable and triage stays pending/retry. No regex placement fallback and no external access grant. Notification failures are visible in metrics, not represented as a confident decline.

## 14. Implementation sequence and ownership

### A. Shared contracts and one complete path — lead

Land domain types, schema, jobs/leases, commit revisions, split read audiences, and a minimal structured decision loop. Wire one message from ingest through independent notification/triage jobs to the new API read and ranking path. Include a fake model transport for deterministic lifecycle tests. This is an implementation milestone on the working branch, not a partial production deployment.

### B. Bounded parallel implementation

- **Lead:** context/tools, agent orchestration, cross-message updates, record integration, configuration and final integration.
- **Notification owner:** fast model contract, logical event arbitration, stable tap identity, race/retry tests; coordinate shared store/API changes through the lead.
- **Surface owner:** FYE and Reading endpoint consumers, desktop/mobile layout, corrections/debug view and onboarding copy; no independent ranking implementation.
- **Evaluation/review owner:** representative fixtures, migration rehearsal, access/race regression review. Once a slot is free, independently review the completed integration.

Each task gets explicit file ownership and acceptance criteria. Shared schema/types are changed by the lead or a single designated owner. Keep commits reviewable by subsystem, even though deployment is one cutover.

### C. Completion gates

- Offline labeled examples measure FYE omissions/noise, Reading recall, action resolution, auth push/access decisions, latency, tool/model calls, and cost. Human corrections are regression cases, not the whole evaluation set. Exact quality targets are set after baseline measurement.
- Unit tests cover ranking contributions/time stability, validation and access. Scripted-model tests cover tools, truncation, failure, rule exceptions and evidence-backed updates.
- Integration tests cover either lane finishing first, retries/restarts, one logical event, actual transport replay limits, open-before-triage, stale commits, pending/restricted derivatives, and migration user-state preservation.
- API/Swift tests cover one server-ordered list, overlapping Records/FYE, permissive Reading, direct auth/pending message opens, and incompatible clients.
- Run repository CI checks (`cargo fmt --all --check`, workspace Clippy/tests, and `passband/test.sh`) plus appropriate client builds. Required external test services must be provisioned or explicitly reported as unavailable.
- Audit remaining references to legacy tier/importance gates and semantic detectors across daemon, API, MCP, TUI, desktop, mobile, and control commands. Old tests asserting removed behavior must be replaced with new product invariants, not blindly kept green.

## 15. Design defaults to validate during implementation

No further product decisions block work. Ranking weights, investigation budgets, the 30-day historical window, late-push acknowledgement behavior, and attachment reader coverage are explicit engineering defaults/proposals, not measured guarantees. Tune them against fixtures and report material behavior changes. Model names and capabilities are resolved from actual deployment configuration before live evaluation.
