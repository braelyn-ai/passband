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
still need the shared transport usage-shape work in #216. #219 explicitly carries
the subject-message truncation/duplication, unmatched rules, and multi-word search
findings; they are not silently declared fixed by this stabilization patch.

## Remaining product review

The following second-review findings remain outside this patch and need separate
resolution before calling the entire rewrite ready to merge: Process mode's empty
new/open lists; Reading/Records done/time-window behavior and missing bill details;
legacy usage/TUI presentation; foreground Mac auth notifications; stale reader focus;
empty kind corrections; cleared shipment visibility; related-thread representation
and dependent-source refresh (also covered in #217); and missing assessment
initialization on the ReadThread evidence path. Do not conflate a green
stabilization suite with resolution of these product issues.

## Verification

- `cargo test -p squelch-core -p squelch-api -p squelch-mcp -p squelchd -p squelch-tui --quiet`: **1,835 passed, 0 failed, 0 ignored**.
- Strict Clippy passed for those five packages with `--all-targets -- -D warnings`.
- `cargo check --workspace` passed; Cargo retains the existing future-compatibility warning for `num-bigint-dig`.
- Full Swift test script passed, including 12 capability-retry checks and 43 notification/tap checks.
- Desktop build passed: 145 sources, version 0.0.7, build 1011.
- Formatting and `git diff --check` passed.

Regressions cover a 1,500-request background backlog with reserved arrival
capacity; ingestion origin versus sender Date; body healing before first
classification; finite job timeout retries without account cooldown; one-call
access execution without sibling context, placement or fan-out; paid malformed
access output; pure human cache probes; persistent human restrictions and sibling
FYE corrections; and coalesced trigger bursts preserving retry state.

No live-inbox evaluation, production rollout, or iOS simulator/device run is
included. Control-plane Postgres integration tests are outside the affected-package
suite and require `SQUELCH_TEST_PG_URL`.
