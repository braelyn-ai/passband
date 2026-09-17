# Agent triage operations

This rewrite switches directly to model-owned triage. There is no shadow pipeline.
The product decisions are in [TRIAGE-REWRITE.md](TRIAGE-REWRITE.md); this file
explains the implementation and tuning surface.

## Execution

Ingest parses and sanitizes MIME, stores the message, and atomically queues work.
It does not classify, seal, infer receipts, close bills, or apply sender rules.
The legacy triage columns hold neutral compatibility values until projected by
an API. They are not the source of destination membership.

Two durable workers operate independently:

- Notification: a small model assesses auth and notification importance. Every
  auth assessment qualifies, including informational login/security alerts.
- Investigation: a bounded agent can read messages, threads, sender history,
  search results, stored records, and text/JSON attachments; optionally request
  a configured review model; then commit an evidence-backed decision.

Explicit model Delivery records also feed carrier polling. Retraction removes
agent-created polling rows when no proposal remains, while preserving existing
legacy tracking and carrier observations. MCP deadlines and shipments read the
canonical decisions and guarded carrier facts.

Binary/PDF attachment extraction is not implemented by the agent evidence reader.
It returns an explicit unsupported result; the prompt prohibits claiming to have
read unavailable evidence. Stored plain text is paged, with Unicode character
positions. Context and tool results have separate byte budgets.

Model output owns categories, Reading/Records membership, thread-level FYE
membership, extracted facts, auth classification, external access, and proposed
revisits. Structural validation checks IDs, evidence, numeric ranges, and observed
revisions. User done/snooze/corrections and completed actions remain authoritative.
Sender rules are supplied as preferences and may be overridden with an evidenced
exception. Memory has a read-only context interface; no memory editor ships.

## Configuration

All fields below are optional TOML settings. These are starting defaults, not
quality claims. Provider credentials and transport resolution continue to use the
existing `[stage2]` connection settings; `[triage.agent].model` overrides its model.
The independent notification model uses existing `[notify]` settings.

```toml
[triage.agent]
# model = "deployment-model"
# review_model = "optional-review-model"
max_model_turns = 4
max_tool_calls = 8
max_review_calls = 1
timeout_secs = 90
concurrency = 2
worker_poll_secs = 1
daily_run_cap = 1000
max_attempts = 6
outage_retry_secs = 300

[triage.context]
initial_thread_messages = 8
max_related_messages = 20
max_context_bytes = 120000
max_tool_result_bytes = 24000

[triage.ranking]
urgency_weight = 30.0
action_need_weight = 25.0
personal_relevance_weight = 20.0
recency_weight = 20.0
waiting_weight = 5.0
importance_weight = 2.0
recency_half_life_hours = 24.0
waiting_saturation_days = 7.0
```

Environment overrides: `SQUELCH_TRIAGE_MODEL`, `SQUELCH_TRIAGE_REVIEW_MODEL`,
`SQUELCH_TRIAGE_MAX_TURNS`, `SQUELCH_TRIAGE_MAX_TOOL_CALLS`,
`SQUELCH_TRIAGE_TIMEOUT_SECS`, `SQUELCH_TRIAGE_CONCURRENCY`,
`SQUELCH_TRIAGE_DAILY_RUN_CAP`, `SQUELCH_TRIAGE_MAX_ATTEMPTS`, and
`SQUELCH_TRIAGE_OUTAGE_RETRY_SECS`. Other agent levers are configured in TOML. Scores order only agent-selected FYE threads;
no weight or importance threshold can add a thread to FYE. Recency uses relevant
message activity, never the time a background job ran.

### Spend and revisit limits

The budget unit is one bounded investigation, including up to `max_model_turns`
provider calls. It is not a token or dollar cap. Reservations atomically enforce
all account, thread, sender, and (when applicable) revisit limits before any call.
The effective account ceiling is the minimum of `triage.agent.daily_run_cap` and
the existing stage-1 and stage-2 global caps. Existing per-tenant usage-page and
warden overrides still apply, as do stage-2 thread and sender caps. With unchanged
defaults, these ceilings are 120 investigations/account/day, 3/thread/day, and
5/sender/day. Raising only the new 1000-run ceiling does not raise the other caps.
`/client/triage-config` exposes the effective agent ceilings and budget unit.

The fast notification lane retains its separate `[notify].daily_cap` and
`SQUELCH_NOTIFY_DAILY_CAP`; investigation backlog cannot consume that allowance.

Agent revisits use existing `[revisit]` settings and their `SQUELCH_REVISIT_*`
overrides: `enabled`, `batch_per_cycle`, `daily_cap`, `max_per_message`,
`max_per_message_lifetime`, `min_lead_hours`, `max_horizon_days`, and
`dedupe_window_hours`. Defaults allow 4 pending revisits, 6 lifetime revisits,
one-hour minimum lead, a 400-day horizon, 12-hour deduplication, and 50 revisit
investigations/account/day, within the shared account/thread/sender ceilings.
Timestamp changes do not evade lifetime limits. The retired deterministic
`deadline_grace_hours` and `fye_stale_days` sweeps do not run in agent triage.

## Recovery and diagnostics

Jobs have leases, attempt counts, availability times, and bounded error codes.
Invalid output and stale commits retry with exponential delay, then remain failed
for inspection and manual re-triage. Observable output constraints get repair
turns within the same investigation before consuming a new attempt. Provider
configuration failures, transport failures, rate limits, server errors, and
timeouts instead leave work pending and retry after the shared outage cooldown;
they do not consume the terminal-attempt allowance. A first-call configuration
rejection refunds its budget reservation. Earlier paid calls remain charged.
Daily budget exhaustion defers work to the next UTC day. Investigations for the
same thread are serialized, including related-source assessment jobs.
Commits compare content, user-state, preference, and evidence revisions within a
transaction. Stale output cannot replace newer user/model state. Passive listing
changes do not invalidate investigations. Worker outcomes appear in
`squelchd_triage_verdicts_total{stage="agent",outcome="..."}`; applied outcomes
also refresh the existing last-success metric.

The fast notification assessment also persists auth classification, score,
reason, model, prompt version, and timestamp for the human inspector.
Applied runs record prompt version, model, configuration, token usage, model-turn
and tool-call counts, and source IDs. Raw provider failures are not persisted as
diagnostics. The decision includes reasons, evidence locations, rule exceptions,
and ranking inputs. No credentials should appear in summaries or notification
text; the model is instructed to describe authentication without reproducing it.

On the first worker pass for an account, cutover queues recent history plus
unresolved/reminded mail silently, skipping messages already queued or assessed.
New arrivals take priority over the migration backlog. It preserves explicit user state and reverses
only identifiable audited legacy bill auto-closures. Historical automatic `done`
rows without reliable provenance cannot be distinguished from a human dismissal
and are conservatively retained. Manual re-triage can reassess chosen messages.

## Surfaces and delivery

`/client/v2/feed` provides the authoritative FYE, Reading, and Records projections.
Reading includes promotional mail by default. Categories and destinations overlap.
`/client/v2/messages/{id}` opens the exact human message while work is pending;
`/opened` acknowledges that message. Human reads do not depend on external access.
The embedded assistant and MCP use separate guarded reads that fail closed for
pending/restricted sources and derivatives of those sources. Reading a legacy
thread through an external agent queues missing access assessments in bounded
batches; ordinary reads never reset exhausted jobs. Human TUI reads remain
available while those assessments are pending.

Record cards read canonical Receipt, Banking, Calendar and Delivery decisions;
marketing cards read promotional Reading decisions. Model date-only deadlines
retain their calendar date and expire at the end of that day in the user's local
timezone. Feed recency clamps future sender dates to trusted local receipt state.
The shredder and Auth pending counts use the current actionable-auth assessment,
not retired `sealed` labels; informational login alerts remain readable.

The full agent can request a notification after a fast decline. Both paths share
one unique arrival-event ledger; opening, completing, or snoozing the message
suppresses a later arrival request. Transport delivery remains at least once:
logical deduplication does not promise exactly-once APNs presentation. The Auth
lookup reads model-assessed actionable auth, but never opens an automatic modal
or emits its own notification. The ordered FYE list has no separate auth insert.

The feedback UX is tracked separately in
[issue #213](https://github.com/braelyn-ai/passband/issues/213).
Live-mail quality and latency evaluation is still needed before judging the
starting model/budget/ranking defaults. Unit and mocked-provider tests verify the
contracts, not the accuracy of a production model on a user's inbox.

## Upgrade order

1. Back up the database consistently, including its WAL, and record deployed versions.
2. Deploy the daemon first. Verify authenticated `GET /client/v2/capabilities`
   advertises `triage_version: 2`, `server_ranked_fye`, `reading`, and
   `pending_message_read`, then check pending-job and provider-error telemetry.
3. Ship the matching desktop and mobile clients. A new client against an older
   daemon displays an explicit daemon-upgrade requirement; there is no legacy
   placement fallback. Notification taps wait for an authenticated connection.
4. Verify a fresh message progresses through notification assessment and triage,
   its push opens the exact message, and actionable auth appears in the shredder.

Rollback requires a compatible daemon/client set and a database recovery plan
that accounts for mail and human actions received after the backup. This worktree
has not been deployed.

## Validation

See [TRIAGE-AUDIT.md](TRIAGE-AUDIT.md) for audit regressions and verification.
Tests use mocked providers. Live-mail accuracy and latency still need evaluation;
a green suite does not establish model quality. Binary/PDF evidence extraction,
provider-specific conversation-prefix caching, and removal of the remaining
legacy compatibility modules are separate follow-ups.
