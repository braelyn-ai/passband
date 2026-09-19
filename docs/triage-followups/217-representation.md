# Thread update representation (issue #217)

Cross-thread attention previously joined the target card to the updating
message's decision. A receipt in thread A could therefore replace thread B's
bill classification, summary and records on the displayed card even though B's
stored message decision had not changed.

The FYE projection now reads classification from its representative message and
keeps the update's evidence in `agent_attention_sources`, independently of the
classification pointer. Both remain checked at the external access boundary. A cross-thread update preserves an existing valid
target representative. If a thread contains only sent or spam messages, attention
keeps a same-thread anchor and the inbound views filter it out; the related update
does not abort the source message's commit. A missing target has no projection to
update. An unclassified inbound representative remains human-visible with its own
subject, a pending reason, and no invented kinds or records. Its update provenance
and external access checks remain intact until assessment is complete.

A human FYE toggle updates the visibility column, the JSON visibility flag, and
the attention revision directly. It does not reinterpret existing attention or
replace its evidence source, representative, action IDs, or activity. This also
works when the representative is unclassified or has subsequently become spam.

The regression checks distinct bill/receipt decisions, target attention changes,
preserved records, and denial when the updating receipt becomes restricted, even
after hide/show clicks. Additional cases cover sent-only/spam-only targets,
visibility toggles before classification, and pending representatives in both
same-thread and related-thread updates, followed by initial classification.
Existing revision and human-state validation remains in the same transaction.

This is a prerequisite for the dedicated thread-refresh executor in #217. It does
not introduce that executor, debounce generations, dependent-source refresh,
rule-save policy changes, or reply-chain cost measurements. Those acceptance
items remain open. No semantic reply-means-done rule is introduced.

The integration keeps #220's source revisions, dependency invalidation and
transitive refresh behavior. The correction regressions compare the entire
attention projection and its source graph before and after each toggle. Context
regressions retained from #223 additionally cover Unicode sibling allocation and
an exact limited initial-source selection; the current base owns their implementation.
