# Context correctness (issue #219)

The initial prompt includes the subject once, followed by recent siblings. The
source snapshot uses the same selection. Matching sender rules and actual contact
membership are exposed; unrelated preferences no longer invalidate an investigation.
Evidence search uses quoted OR terms with BM25 ranking, retaining account, sent,
and spam filters. This is internal evidence access, not external-agent permission.

Text allocation measures serialized JSON, including escaping and truncation
markers. It preserves the subject first, then shares remaining space between
other text fields. Metadata that alone exceeds the limit still fails closed.
Existing character-offset message paging can retrieve omitted body text.

The synthetic nine-message fixture retains a 23,924-byte subject, including its
final decisive sentence, within a 30,000-byte context. This demonstrates input
allocation, not provider billing savings or classification accuracy.

Remaining issue scope: provider/gateway prefix-cache capability and billing
verification, multi-turn measurements, and removal of retired execution modules.
No new cache mechanism is enabled by this change. Existing system-prompt caching
remains provider-specific; uncached execution is still supported. Legacy
stage1/stage2/router/revisit modules still have config, Store, extraction-type and
test dependencies and cannot simply be deleted wholesale. This PR does not close
#219 or claim the Rust/Swift acceptance matrix for the eventual deletion.
