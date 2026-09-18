# Thread update representation (issue #217)

Cross-thread attention previously joined the target card to the updating
message's decision. A receipt in thread A could therefore replace thread B's
bill classification, summary and records on the displayed card even though B's
stored message decision had not changed.

The FYE projection now reads classification from its representative message and
returns the update's source separately as provenance. Both remain checked at the
external access boundary. A cross-thread update preserves an existing valid
target representative; a target with no inbound message is rejected instead of
using an unrelated fallback. First-time attention without a classified target is
stored, but no unrelated classification is borrowed to make it displayable.

The regression checks distinct bill/receipt decisions, target attention changes,
preserved records, and denial when the updating receipt becomes restricted.
Existing revision and human-state validation remains in the same transaction.

This is a prerequisite for the dedicated thread-refresh executor in #217. It does
not introduce that executor, debounce generations, dependent-source refresh,
rule-save policy changes, or reply-chain cost measurements. Those acceptance
items remain open. No semantic reply-means-done rule is introduced.
