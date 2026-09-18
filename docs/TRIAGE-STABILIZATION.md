# Triage stabilization after the second review

Keep first classification working, stop redundant paid work, and preserve human
restrictions. This is a bounded stabilization patch, not the larger billing and
scheduler redesign. The first rewrite checkpoint is `4b7099d` on `redo-triage`.
Nothing in this document authorizes deployment or merging.

## Implemented scope

- Human corrections survive content changes. Human restrictions are checked
  directly at the external read boundary as well as reapplied on model commits.
  FYE corrections are thread-scoped so sibling and related updates cannot undo them.
- Human thread reads and cache checks are pure. Only an explicit external thread
  request schedules missing historical access assessments.
- Access work has its own executor: one call to the configured small notification
  model, one message, no tools, placement, records, attention or revisit output.
  It writes access state and audit metadata only. Oversized input fails closed.
- Immediate investigation requests coalesce by account/message/content revision.
  A leased job accumulates at most one follow-up; retries absorb it while retaining
  backoff and attempts. Trigger changes do not resurrect failed work. New explicit
  manual re-triage is allowed, and repeating that same request is idempotent.
- Total investigation cap stays explicit (default 1000/day). Background work has
  a sub-cap (default 200/day), leaving 800 runs protected from background spending.
  No old escalation cap gates first classification. Sync origin distinguishes live
  incremental arrivals from initial backfill independently of push eligibility.
- Whole-investigation/access timeouts consume normal retry attempts. They do not
  trip the account's provider-outage circuit or refund uncertain provider spend.
- Capability checks retry transient network/server failures, including continued
  boot recovery after short retries. Account switches preflight the target before
  changing active account state.
- Failed notification account switches park the tap until a new connection or
  explicit tap. Completing the failed drain cannot immediately retry it.

See [TRIAGE-OPERATIONS.md](TRIAGE-OPERATIONS.md) for the effective configuration.

## Deferred architectural work

Each issue begins with a short technical description, then includes motivating
failures, relevant code, proposed scope, acceptance criteria, and non-goals.

1. [#216: Per-call accounting and cost budgets](https://github.com/braelyn-ai/passband/issues/216)
2. [#217: Dedicated coalesced thread-refresh executor](https://github.com/braelyn-ai/passband/issues/217)
3. [#218: Fair scheduling and provider recovery](https://github.com/braelyn-ai/passband/issues/218)
4. [#219: Context allocation, prefix reuse, and retired-code cleanup](https://github.com/braelyn-ai/passband/issues/219)

Run/turn caps are not dollar caps. Provider failures such as truncation/refusal
still need the shared transport usage-shape work in #216. Dedicated thread-refresh
execution remains in #217; prefix reuse and retired-code cleanup remain in #219.
The evidence and provenance correctness fixes below are implemented here.

## Product review fixes

- Evidence allocation preserves the target message before trimming sibling text,
  removes the duplicate target, and measures actual encoded bytes. Context includes
  matched sender rules and contact status. Search tries exact FTS matching first,
  then falls back to partial multiword matches.
- Related-thread updates keep the target message identity and classification.
  Separate attention provenance gates external access; existing beta rows are
  repaired without dropping those guards. Changed sources queue coalesced refreshes
  of direct and transitive dependents. ReadThread initializes only exposed sources.
- Process uses canonical FYE order. Reading/Records exclude done items and default
  to the last 30 days; explicit API queries can request another window or all time.
  Bill cards show amounts, due dates, and autopay. The TUI reads canonical summaries
  and server order, and usage reporting distinguishes pending from classified mail.
- Auth events carry an explicit persisted flag for foreground banners and sound.
  The relay still carries only an event ID. Reader focus is consumed once; empty
  category corrections are rejected, with model repair before committing an empty
  effective classification. MCP hides cleared shipments.
- Main's guided practice and outgoing attachments are preserved. Practice supports
  v2 feeds and reading; real notification taps wait until practice ends. Restricting
  an external agent preserves human drafts and their attachments. Explicit draft
  deletion retains attachment cleanup.

The PR remains a draft for live-inbox quality and latency evaluation before rollout.

## Verification

- `cargo test -p squelch-core -p squelch-api -p squelch-mcp -p squelchd -p squelch-tui --quiet`: **1,889 passed, 0 failed, 0 ignored**.
- Strict Clippy passed for those five packages with `--all-targets -- -D warnings`.
- `cargo check --workspace` passed; Cargo retains the existing future-compatibility warning for `num-bigint-dig`.
- Full Swift test script passed: 36 suites, including capability recovery, notification/tap behavior, canonical feeds, reader focus, and rehearsal isolation.
- Desktop build passed: 158 sources, version 0.0.7, build 1023.
- iOS generic simulator build passed for the app and notification extension (code signing disabled).
- Formatting and `git diff --check` passed.

Regressions cover a 1,500-request background backlog with reserved arrival
capacity; ingestion origin versus sender Date; body healing before first
classification; finite job timeout retries without account cooldown; one-call
access execution without sibling context, placement or fan-out; paid malformed
access output; pure human cache probes; persistent human restrictions and sibling
FYE corrections; and coalesced trigger bursts preserving retry state.

No live-inbox evaluation, production rollout, or simulator/device runtime session is
included. Control-plane Postgres integration tests are outside the affected-package
suite and require `SQUELCH_TEST_PG_URL`.
