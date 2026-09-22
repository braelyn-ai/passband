# Paid-response accounting (issue #216)

A provider response now retains its reported usage even when the model refuses,
truncates, omits output, returns invalid JSON, or fails caller validation. Agent,
access, and notification consumers collect usage before interpreting the outcome.
A truncation returns to the caller instead of silently making a second paid call.
Repair calls therefore consume the agent's explicit turn allowance.

Missing usage is unknown, not zero. Transport timeouts and response decoding
failures still cannot establish provider charges. The existing job budget keeps
uncertain reservations; this transport change does not invent a refund or an
estimated dollar value. Returned input/output/cache usage is counted once on the
existing success/failure paths, but an interrupted ledger write is not a durable,
idempotent settlement protocol.

This PR is the transport prerequisite for #216, not the complete cost-budget
redesign. Remaining work includes configured prices and output limits, per-call
atomic monetary reservations for arrival/background/notification allocations,
idempotent settlement and crash recovery, original-day accounting, budget-wait
visibility, and deliberate usage/control/warden migration. No provider billing
ceiling or dollar-budget guarantee is claimed, and the issue remains open.
