# Triage audit follow-up

This records fixes to the pre-merge audit. The worktree is not deployed.
Operational settings and daemon-first rollout are in
[TRIAGE-OPERATIONS.md](TRIAGE-OPERATIONS.md).

## Findings addressed

| Finding | Result |
| --- | --- |
| Missing related attention row caused endless stale commits | Missing revision is zero. Stale commits retain their lease so normal attempt accounting and exponential backoff apply. Investigations serialize per thread. |
| Unbounded timestamp-based revisits and inactive spend controls | Enforce revisit lead/horizon, deduplication, pending and lifetime limits across revisions. Atomically reserve account/thread/sender/revisit budgets; existing tenant cap overrides apply. New agent cap has an environment override. |
| Provider outage permanently orphaned mail | Provider/configuration outages defer without consuming terminal attempts and share a cooldown. First-call configuration rejections refund reservations. Regression simulates eight rejections followed by recovery. |
| Ingest queued behind migration and queued twice | Real `ingest` arrivals outrank migration. Cutover skips already queued/assessed messages. |
| Shredder and Auth counts depended on obsolete sealed labels | Both use current actionable-auth assessments. Human reads/actions no longer depend on legacy sealing guards. |
| Rescued spam had no placement | Clearing spam atomically enqueues a `not_spam` investigation. |
| Record cards depended on abandoned extractors | Swift card adapters consume canonical Records and promotional Reading. Generic facts receive generic labels rather than invented specialist detail. |
| Delivery projection used raw tracking numbers | Projection and MCP carrier enrichment normalize tracking identifiers and deduplicate equivalent spellings; retraction uses the same key. |
| Historical siblings blocked external access forever | Only consumed evidence gets provenance, and historical evidence gets access-assessment jobs. External thread reads request missing assessments in bounded batches; they never reset exhausted jobs. |
| Hidden search hits consumed the entire result limit | External search expands retrieval until enough allowed results are found or candidates are exhausted. The semantic leg retains sqlite-vec's native 4096-candidate ceiling. |
| TUI opened blank pending/restricted threads | TUI uses the human read path. External agents retain separate access checks. |
| Pending list corrections froze future placement and raced commits | Add/remove deltas are applied atomically and retained per choice, preserving unrelated future agent decisions. |
| Cold-launch push taps were dropped | Tap targets wait for an authenticated connected client, including account switching. |
| New client silently failed against an old daemon | Capability probe presents an explicit daemon-upgrade requirement. Rollout documentation specifies daemon first. |
| Reader adoption acknowledged unseen arrivals | Open acknowledgement waits for geometry-confirmed visible focus rather than adoption/index changes. |
| Passive mail listing discarded paid investigations | Revision fingerprints ignore the passive new-to-open status transition; explicit human changes still invalidate stale work. |
| Fast lane used false contact state and every sender rule | Inputs contain real contact membership and only sender-matched rules. |
| Invalid final output failed only inside commit | Observable evidence/action/related-thread constraints participate in bounded model repair turns before commit. Invalid JSON also retains provider token usage. |
| Evidence included spam/sent and scanned every body | Evidence excludes spam/sent mail and searches the FTS index. |
| Date-only deadlines expired a day early | API preserves `deadline_date`; client evaluates the end of that local calendar day. |
| Forged future dates pinned feeds | Ordering clamps sender dates against stable locally recorded arrival state. |
| Worker disappeared from metrics | Applied/stale/retryable/failed/deferred outcomes emit through the existing verdict metric family with `stage="agent"`; success refreshes the existing success timestamp. |
| Content revision canceled eligible arrival push | Unfinished notification eligibility transfers to the replacement revision. |
| Empty queues caused redundant claim scans | Claim loops stop at the first empty result; ready/leased lookup indexes support the new queue. |
| Design documented ignored TOML blocks | Examples now use actual `[notify]` and `[sync]` settings and document active revisit controls. |

The reported FYE/standing-reminder mismatch was refuted and is not treated as a
fix. No additional semantic classifier or regex fallback was introduced.

## Verification

- `cargo test -p squelch-core -p squelch-api -p squelch-mcp -p squelchd -p squelch-tui --quiet`:
  **1,823 passed, 0 failed, 0 ignored** after the final code changes.
- Strict Clippy passed for those five packages with `--all-targets -- -D warnings`.
- `cargo check --workspace` passed. Cargo reports an existing future-compatibility
  warning for the `num-bigint-dig` dependency.
- `passband/test.sh` passed all 28 suites after the client fixes.
- `passband/build.sh` compiled 145 sources and produced the signed desktop app.
  Edited iOS views passed Swift syntax parsing and the Xcode project passed
  plist validation; no iOS simulator/device build was run.
- Formatting and `git diff --check` passed.
- The unrelated control-plane integration tests require `SQUELCH_TEST_PG_URL` and
  are outside the successful affected-package test count.

Regression coverage includes queue collisions, stale evidence, finite revisit chains,
atomic budget reservations/refunds, outage recovery, same-run output repair,
current-revision shredding, rescued spam, correction races, historical access,
notification replacement, date precision, and normalized carrier projections.

## Remaining work and limits

- Live-inbox accuracy, latency and actual model cost need evaluation. The enforced
  unit is a bounded investigation, not a dollar/token cap; one investigation can
  include multiple provider calls up to the configured turn limit.
- Provider-specific conversation-prefix caching is not implemented by this audit
  patch. Existing provider caching behavior remains; repeated context can still
  contribute to billed input.
- Retired stage/router/revisit modules and compatibility Store methods remain.
  Their deterministic semantic workers do not run in the new pipeline. Removing
  that code should be a separate cleanup with compatibility consumers checked.
- Binary/PDF attachment extraction remains unsupported. The agent receives an
  explicit unsupported result.
- External access remains pending if assessment exhausts attempts; repeated reads
  do not bypass retry limits. Manual re-triage is the recovery path.
- No production rollout or iOS simulator/device validation was performed here.
