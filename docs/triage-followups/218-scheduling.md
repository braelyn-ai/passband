# Queue claims and recovery (issue #218)

Claims explicitly use a partial index containing queued and leased jobs only.
Completed history is excluded from the candidate index. Expired leases remain
claimable, and active sibling leases still exclude concurrent work on a thread.
New arrivals retain claim priority. Other work uses available-at/id ordering so
new manual/access work cannot indefinitely jump ahead of older migration work.
Retries retain their existing attempt and availability state.

The refine worker doubles its empty-poll delay up to 30 seconds. Ingest wakes it
immediately and resets the delay. Due work from other sources is discovered
within that bounded interval; this is not an exact next-due notification system.
Notification polling is unchanged.

After a provider cooldown, the account admits one investigation as a recovery
probe. A successful response clears the expired circuit; an outage extends it.
Budget deferrals do not reopen the circuit. Whole-investigation timeouts remain
job-local and retain their finite attempt limit. Circuits remain per account,
not a fleet-wide provider/configuration registry.

The regression fixture contains 200,000 completed jobs and a 1,500-job migration
backlog, with a new arrival and 100 concurrent human message reads. It checks the
actual claim SQL with EXPLAIN, verifies arrival ordering, and times 100 claims.
The deterministic guard is use of the partial index. A generous five-second
aggregate threshold (50ms mean in a debug build) catches the previously reported
74–92ms history scan while allowing substantial machine contention. This is
synthetic local evidence, not a production service-level target.

Remaining #218 scope includes protected execution slots, per-class/hot-sender
fairness under sustained arrival load, provider/configuration-wide probe sharing
and jitter, richer wait/latency telemetry, and production cutover measurements.
Background can still fill a batch and delay arrivals until it finishes. This
initial PR must not be described as completing execution-capacity isolation or
closing #218.

Local debug measurement on 2026-09-17: 100 claims and completions with the
1,500-job backlog and concurrent reads took 365.5ms (3.65ms mean), versus 11.8ms
(0.118ms mean) in the initial nearly-empty-queue history-only fixture. This
separates pending-backlog cost from completed-history cost; neither is a fleet
benchmark.
