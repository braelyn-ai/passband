# Agentic triage rewrite — working plan

Status: product direction agreed; [concrete code design](TRIAGE-CODE-DESIGN.md) drafted for implementation. Numeric tuning defaults and engineering choices are documented there for evaluation.

## Agreed direction

- Models own semantic classification, placement, sensitivity, and escalation. Remove deterministic semantic detectors, provisional verdicts, contact score floors, category clamps, and client-side placement heuristics.
- Internal model calls may read authentication mail. Sealing restricts access by the user's external agents, not internal triage or human-facing placement.
- Seal only actionable authentication mail containing access-granting or account-recovery material, such as 2FA codes, password-reset links, magic sign-in links, or actionable verification links. Informational login/security alerts remain available to external agents unless they also contain such material. All auth still pushes regardless of sealing.
- Keep a parallel small-model notification path. All auth, including login/security alerts, always pushes; other mail pushes according to notification importance. Full triage may later request a push that the fast path declined or failed to decide.
- Notification taps open the email immediately, including while triage is pending. Placement waits for the completed triage decision; this timing difference is intentional.
- Importance remains central to the notification gate but becomes secondary for FYE ordering, with no score threshold deciding FYE membership.
- FYE is one ordered list, not sections. Recency is an explicit ordering factor alongside urgency, unresolved actions, and personal relevance.
- Categories describe the email; destinations describe where it appears. Destinations can overlap: a receipt can remain in Records while an associated problem appears in FYE.
- Prioritize readable code and explicit configuration levers so behavior can be tuned during beta without accumulating scattered special cases.
- Rename the Newsletters display destination to Reading. Include editorial content and sales promotions by default; lean permissive and let users curate.
- Give sender rules to the agent as high-priority preferences. The agent may override them when strongly justified; they are no longer unconditional post-model overrides.
- Leave an interface for future agent-curated long-term memory. Do not implement memory storage, learning, or editing in this rewrite.
- Cut over to the new triage system with the release. No production shadow deployment or gradual cohort rollout is required for the small beta population.

## Decision model

Separate email kind, required action, current state, destination, relevant timing, and external-agent access. Persist a concise reason and supporting message references for meaningful decisions. Preserve explicit user state such as done, snooze, and reminders separately from inferred state.

Proposed kinds: correspondence, editorial, promotional, bill, receipt, financial update, delivery, event/reservation, account/service, authentication/security, and general/unknown. Allow mixed content where useful. Payment required, autopay confirmed, and payment resolved are facts/state, rather than categories that force placement.

Destinations are independent of kind and sensitivity and may overlap. A receipt with a problem may appear in both Records and FYE; a bill paid automatically may only need Records. Destinations are FYE, Reading, and Records, with quietly filed meaning no surfaced destination. The code design proposes multiple kinds per message, message-level Reading/Records placement, and one authoritative thread-level FYE item.

## Readability and configuration

Treat understandable implementation and easy tuning as core requirements, not cleanup after the rewrite. Prefer small, plainly named modules and typed contracts for ingest, notification assessment, agent context/tools, triage decisions, ranking, access enforcement, and delivery. Keep orchestration readable from top to bottom. Avoid a generic workflow framework or a second hidden classifier in helper functions.

Centralize tunable policy in typed configuration with documented defaults, units, valid ranges, and examples of the behavior each setting changes. Group settings by purpose:

- **Models and investigation:** model selection for each path, reasoning effort where supported, tool-call and context budgets, timeouts, concurrency, and retry limits.
- **Notifications:** non-auth importance threshold, freshness window, and delivery/retry settings. All-auth notification eligibility is an agreed product invariant, not an ordinary score weight.
- **FYE ranking:** explicit contributions for recency, time sensitivity, unresolved actions, personal relevance, waiting time, and secondary importance. The exact ranking method and numeric defaults require evaluation. If using a scoring function, expose named contributions and their units rather than scattering arithmetic through SQL or clients. Ranking never changes model-decided destination membership.
- **Agent policy:** versioned, readable prompts and decision schemas; clearly stated category definitions, permissive Reading defaults, and the strength of user preferences. Avoid code-level keyword exceptions.
- **Revisits and migration:** revisit budgets and scheduling limits, historical re-triage window, and migration batch sizes.

Only expose settings with an actual behavioral use; do not prebuild a configuration UI or speculative configuration framework. Validate configuration at startup. Record the applicable model, prompt/schema version, and configuration version with decisions so evaluations can explain changes. Keep diagnostics concise and avoid copying authentication secrets or full email bodies into logs.

Make ranking inspectable: show the factors that placed an item where it is, distinct from the model's reason for including it in FYE. Use representative fixtures to compare configuration changes and meaningful tests for policy boundaries, races, and access enforcement. Document common tuning tasks alongside the settings. Keep access controls, delivery deduplication, and preservation of explicit user actions as invariants rather than adjustable heuristics.

## Concurrent paths

Ingest stores mail as pending and starts two independent jobs:

1. **Fast notification model:** receives message content and bounded relevant preferences. Returns auth classification, notification importance, safe push text, and a brief decision reason. Auth classification causes a push regardless of the score or sender preference. Other messages use the configured notification threshold. This path does not decide final placement or wait for full triage.
2. **Triage agent:** reads message and relevant thread context/preferences, retrieves more evidence or requests stronger reasoning when useful, then commits a coherent decision. It can request a notification after learning more, including when the fast model missed auth.

Use a shared durable notification ledger/outbox so both paths can request delivery without duplicate events. Handle either completion order, retries, crashes, and fast-path failure. A fast decline is not a permanent veto; an already delivered push cannot be undone. Later reminders or materially new developments need a distinct notification identity, rather than replaying the arrival push.

Separate notification importance from general triage importance in schema and prompts. Remove heuristic seed dependency and known-contact floors from the fast path. Model failures remain undecided and retry or fall through to the deliberate path; do not reinstate regex classification as an outage fallback.

All auth pushes for new incoming mail, including codes, reset links, magic links, verification requests, and login/security alerts, subject to the user's global notification controls and OS delivery. Historical migration must not replay arrival pushes. Expiry behavior and late-notification freshness remain decisions to settle.

Notification payloads reference a stable message identity. Tapping opens that email through the human-facing read path without depending on triage completion, destination membership, or external-agent access clearance. Merely opening mail does not mark its obligations resolved. The agent can finish triage while the user reads; applying its result must preserve any intervening explicit user actions. If the user leaves the notification for later, triage has time to finish placement normally. No provisional placement is needed to support notification delivery or navigation.

## Agent tools and context

Tools retrieve thread history, relevant messages, attachments, sender interactions, and user corrections. They return evidence, not heuristic verdicts. Email content is untrusted evidence; it cannot alter user preferences or grant access.

The agent receives explicit sender rules with their scope and provenance. Following a rule is the default; an override must record the rule, the contrary evidence, and why the exception matters. An exception does not silently rewrite the rule. Example: promotional mail is usually suppressed for a sender, but a service cancellation from that sender needs attention.

Future memory seam: a versioned context-provider interface, currently supplying no learned memory, and a separately gated future memory-edit capability. Future entries should support scope, provenance, revision, and deletion. Keep learned memory distinct from explicit user preferences and current thread state. Reserve the interfaces; do not ship an empty memory UI or active write tool.

## FYE and Reading

FYE membership is an explicit model decision about needed or meaningful attention, including personal correspondence without a task. Present one ordered list, without attention-state sections. Rank using time sensitivity, unresolved commitments, who owes the next move, meaningful changes, personal relevance, recency, and waiting time. Importance is secondary. Recency should help fresh mail surface while meaningful older obligations can retain priority. Proposed thread-level recency uses the latest relevant incoming development, rather than the time a background re-triage ran. The exact ordering implementation remains open; avoid introducing a new collection of hidden classification rules in sorting SQL.

Track attention at the thread or situation level so a payment confirmation can resolve a bill and multiple delivery updates need not become separate priorities. Use bounded evidence retrieval and revisit on relevant new messages or scheduled times.

Reading includes newsletters, editorial content, announcements, and sales promotions by default. Sender rules inform curation; strongly justified agent exceptions remain possible. Rename the destination consistently across desktop, mobile, onboarding, and accessibility copy, and replace client heuristics with the stored placement.

## External-agent access

Enforce access independently of human visibility and push eligibility. Pending sensitivity decisions are unavailable to external agents. Only the authoritative triage access decision may grant access; the notification model cannot do so. Restricted content must remain excluded from agent-facing bodies, summaries, search results, and derived records. Internal processing is allowed. Review mixed-sensitivity threads and existing whole-thread exclusion behavior explicitly.

The restriction applies to actionable authentication material that can grant access, verify identity/account ownership, or recover an account: codes, password-reset links, magic sign-in links, and equivalent credentials. A login alert, password-change confirmation, or informational security notice without such material is not sealed merely because its category is authentication/security. A generic link to account settings is not itself a credential. Mixed-content mail carrying an actionable auth credential is restricted. The agent assesses these distinctions from content; do not reintroduce keyword or URL-shape detectors. Test informational alerts and credential-bearing variants separately. All of these auth categories remain eligible for automatic push.

## Rewrite and validation

1. Finalize decision schema, FYE ordering, tool budget, and remaining notification lifecycle details.
2. Build an offline evaluation set from representative and corrected mail. Measure missed attention, unwanted FYE placement, Reading quality, auth detection, access mistakes, notification decisions, latency, and cost. Compare capable single-pass triage with selective investigation.
3. Implement the new decision store/interfaces, notification contract and shared delivery arbitration, agent context/tools, and access enforcement.
4. Update API consumers and all display surfaces, including corrections and decision explanations.
5. Cut over on release. Version decisions and re-triage existing mail within an agreed history window, preserving explicit user state and preventing historical push floods. Keep a recoverable database backup and a documented rollback procedure compatible with the schema change.
6. Delete superseded semantic pipelines and update security/notification documentation to reflect the new internal-model boundary.

Checks must cover fast decline followed by agent push, both lane completion orders, duplicate/retried requests, all auth types including login alerts despite sender suppression, notification taps before and after triage completion, preservation of user actions taken while triage is pending, provider failures, restricted-content derivatives, user-rule exceptions, resolved obligations, mixed-content mail, and migration preservation of user state.

## Follow-up UX issue

[Issue #213](https://github.com/braelyn-ai/passband/issues/213) tracks the agent asking whether the user wants to keep receiving "emails like this," offering keep, suppress sender, and suppress similar content across senders. Suppression controls Passband placement/notifications; unsubscribing is a separate explicit action. This feedback UI and long-term learned memory are outside the rewrite scope.

## Engineering defaults to evaluate

- The code design specifies initial ranking weights, category contracts, investigation budgets, notification lifecycle defaults, and historical re-triage window. Validate and tune these during implementation; the product direction is settled.
