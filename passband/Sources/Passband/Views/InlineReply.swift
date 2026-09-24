// REPLY WHERE YOU READ. A pinned composer under the message stack rather than a
// panel over it: the email you are answering stays on screen, unblurred and
// scrollable, which is the whole point of answering from the reader.
//
// The ceremony is the pane composer's, and LOCKED: ⌘Enter closes the reply and
// holds it five seconds behind an undo toast (`AppStore.sendWithUndo`), then it
// goes out ONCE WITHOUT override. A blocked verdict reopens the reply with the
// guard's kinds, and only that unlocks ⌘⇧Enter to send anyway. Both composers
// hand the same state to the same hold, so there is one request shape and one
// error mapping. The body is markdown, live-styled by the same
// MarkdownTextView the pane uses.
//
// THE HEADER LINE IS A DISCLOSURE. Collapsed it says who this reply reaches, in
// one line, because that is all most replies need. Opened it is three editable
// recipient fields — to, cc, bcc — so the answer to "actually, put Dana on bcc"
// is two clicks in the composer you are already in, rather than closing it and
// starting the mail again somewhere that has the fields.
//
// WHICH MEANS THE AUDIENCE CHANGES HANDS. The composer still opens knowing only
// its parent, and the real set is still derived server-side — the parent's
// Reply-To, the room a reply-all widens to. What is new is that the derivation
// is SEEDED into the fields when it lands, and from that moment the fields are
// the answer and the send carries them explicitly (`recipientsStated`). Before
// it lands, and if it never does, the wire carries no recipients at all and the
// daemon derives exactly as it always has — which is why a failed lookup costs a
// preview and never a reply.
//
// The seeded set is also remembered (`seededRecipients`), so the autosave can
// tell "a reply nobody addressed" from "a reply somebody moved to bcc": the
// first must not mint a draft, the second must.
//
// ON A PHONE IT IS THE SAME BAR, pinned by `.safeAreaInset(edge: .bottom)`
// instead of by a VStack — so the keyboard lifts it and insets the mail behind
// it rather than squashing the reader (see ThreadViewer). Two things are fenced.
// The KEY HINT BAR becomes real buttons, and that is not decoration: the
// ceremony is driven ENTIRELY by keys on the Mac, so without them a phone could
// open a reply and have no way on earth to send it. And the editor is
// SHORTER, because 150pt of text view above a raised keyboard leaves nothing of
// the email you are answering — which is the whole reason this composer is here
// and not a panel over it.

import SwiftUI

struct InlineReply: View {
    /// The thread's messages, so the draft's `reply_to_message_id` can be resolved
    /// back to the message it answers. Passed in rather than read from the store
    /// because the composer is mounted unconditionally — see ThreadViewer.
    let messages: [ClientMessage]
    /// The thread's subject, for the derived-subject line. Messages carry no
    /// subject of their own on the wire.
    let threadSubject: String

    @Environment(AppStore.self) private var store
    @FocusState private var focusedField: RecipientSlot?

    /// Whether the recipient fields are open. Per-composer by nature: it resets
    /// when the reply closes, because the next one is a different audience.
    @State private var editingRecipients = false
    /// A file is being dragged over the composer: the editor's well says so.
    @State private var dropTargeted = false

    /// The daemon's derived recipient set, TAGGED with the key it was fetched
    /// for. The tag is what makes a stale read impossible: this view is
    /// mounted unconditionally, so bare state would survive one composer and
    /// render — for a frame, before the keyed task clears it — under the next.
    /// nil = nothing landed yet; `(key, nil)` = the fetch for that key failed.
    @State private var fetchedRecipients: (key: String, set: ReplyRecipients?)?

    private var compose: ComposeState? { store.inlineReply }
    private var guarded: Bool { !(compose?.guardKinds.isEmpty ?? true) }
    /// The message being answered. nil = nothing to answer, which renders as no
    /// composer at all.
    private var parent: ClientMessage? {
        guard let id = store.inlineReply?.replyToMessageId else { return nil }
        return messages.first { $0.id == id }
    }
    /// What the daemon will title the reply, mirrored for display.
    private var replySubject: String { ComposeCopy.replySubject(threadSubject) }

    /// The fetch's identity: which parent, in which mode. EVERY reply fetches
    /// now, not just reply-all — the daemon honors the parent's Reply-To on a
    /// plain reply too, so the stored sender the client holds can be the wrong
    /// answer. Mode is part of the key so `r` then Enter on the same message
    /// refetches rather than showing the other mode's set.
    private var recipientsKey: String? {
        guard let compose = store.inlineReply, let parent = compose.replyToMessageId else {
            return nil
        }
        return "\(parent):\(compose.replyAll)"
    }

    /// The fetched set, only if it belongs to the CURRENT key.
    private var recipients: ReplyRecipients? {
        guard let fetchedRecipients, fetchedRecipients.key == recipientsKey else { return nil }
        return fetchedRecipients.set
    }

    /// True once the current key's fetch has come back (with or without a set)
    /// — the difference between "deriving…" and "the daemon will derive it".
    private var recipientsSettled: Bool {
        fetchedRecipients?.key == recipientsKey
    }

    var body: some View {
        if let compose, let parent {
            VStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 9) {
                    headerLine(compose, parent: parent)
                    if editingRecipients {
                        recipientEditor(compose)
                    }
                    editor(compose)
                    // The files, under the editor. Nothing when empty.
                    AttachmentTray(slot: .inlineReply)
                    // A held send came back blocked.
                    if guarded { GuardVerdictBox(kinds: compose.guardKinds) }
                    if let error = compose.error {
                        Text(error).font(Typo.micro).foregroundStyle(Palette.danger)
                    }
                }
                .padding(.horizontal, Self.gutter)
                .padding(.top, 12)
                .padding(.bottom, 10)
                // Anywhere on the composer, same as the pane: see
                // `composeDropTarget`.
                .composeDropTarget(.inlineReply, targeted: $dropTargeted)

                #if os(macOS)
                    KeyHintBar(hints: hints)
                #else
                    actionBar(compose)
                #endif
            }
            // The reader's own measure, so the composer sits under the column it
            // answers rather than sprawling the full window width. A phone is
            // narrower than the column will ever be, so there it is inert.
            .frame(maxWidth: ThreadViewer.columnWidth, alignment: .leading)
            .frame(maxWidth: .infinity)
            // A GROUND OF ITS OWN on the phone, and only there. On the Mac this
            // bar sits INSIDE the reader's own material, at the bottom of its
            // stack; as a safe-area inset it floats over scrolling mail, and mail
            // reading through the send button is not a composer.
            #if !os(macOS)
                .passbandGlass(.pane, cornerRadius: 0, tint: Palette.glassTintStrong)
            #endif
            .overlay(alignment: .top) { Hairline() }
            // REGISTRATION ORDER IS LOAD-BEARING. Within a context the LATEST
            // registered set wins, and this set mounts with the composer — after
            // the viewer's — which is the only reason Escape here means "leave the
            // composer" while the viewer's Escape still means "leave the email".
            // Hoisting these onto the always-mounted viewer would register them
            // FIRST and invert that layering: Escape would close the thread out
            // from under an open draft.
            .keyBindings(.thread, bindings)
            // Once per (parent, mode). Keyed, and the fetched value carries its
            // key too, so reopening the composer on another message or in the
            // other mode can never show the last one's addresses — not even for
            // the frame before this task runs.
            .task(id: recipientsKey) { await loadRecipients() }
            // A different message (or the same one in the other mode) is a
            // different audience: the fields close so nobody edits one reply's
            // recipients believing they are another's.
            .onChange(of: recipientsKey) { _, _ in editingRecipients = false }
        }
    }

    /// Ask the daemon who this reply would reach. Best-effort by contract: the
    /// send derives the set again server-side (and, for a reply-all, hard-fails
    /// there if it cannot), so a failure here is a missing preview, never a
    /// blocked reply — which is why it neither surfaces an error nor touches
    /// `compose.error`.
    private func loadRecipients() async {
        guard let key = recipientsKey, let compose = store.inlineReply,
            let parentId = compose.replyToMessageId
        else { return }
        let slot = compose.id
        let fetched = try? await APIClient.shared.replyRecipients(parentId, all: compose.replyAll)
        // The composer may have closed, or moved on, while this was in flight.
        guard recipientsKey == key else { return }
        fetchedRecipients = (key, fetched)
        seed(fetched, into: slot)
    }

    /// HAND THE DERIVED SET TO THE COMPOSER, once, and only while the composer
    /// has not been addressed by anybody yet.
    ///
    /// This is the moment the audience changes hands: before it, the send
    /// carries no recipients and the daemon derives; after it, the fields are
    /// the answer. Seeding what the daemon itself just derived means the two are
    /// the same mail — nobody's reply is quietly re-addressed by the handover.
    ///
    /// Keyed to the composer's identity, like every other write that lands after
    /// an await: the slot may hold a reply to another message by now, and
    /// stamping one message's recipients onto another's draft is the worst
    /// available outcome. `recipientsStated` already being true means either a
    /// restored draft or the sender got here first, and both outrank a
    /// derivation.
    private func seed(_ derived: ReplyRecipients?, into slot: UUID) {
        guard let derived, var next = store.inlineReply, next.id == slot,
            !next.recipientsStated
        else { return }
        let set = Recipients(to: derived.to, cc: derived.cc ?? "")
        next.recipients = set
        // Remembered so the autosave can tell an untouched reply from an
        // addressed one — see `ComposeState.seededRecipients`.
        next.seededRecipients = set
        next.recipientsStated = true
        store.inlineReply = next
    }

    // MARK: - panes

    private func headerLine(_ compose: ComposeState, parent: ClientMessage) -> some View {
        HStack(spacing: 5) {
            // THE WHOLE "replying to <who>" PHRASE IS THE DISCLOSURE, chevron
            // and all: the thing you want to change is the thing you click.
            Button {
                editingRecipients.toggle()
                if editingRecipients { focusedField = .to }
            } label: {
                HStack(spacing: 5) {
                    Image(
                        systemName: editingRecipients
                            ? "chevron.down" : "chevron.right"
                    )
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(Palette.inkFaintest)
                    Text(compose.replyAll ? "replying to all" : "replying to")
                        .font(Typo.micro)
                        .foregroundStyle(Palette.inkFaintest)
                    // Addresses and sender strings alike are email-derived:
                    // rendered as Text only, never as markup, and never
                    // interpolated into a localized literal.
                    Text(headerTarget(compose, parent: parent))
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(Palette.inkDim)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            // A sentence in the micro voice reads as a caption until the
            // pointer says it is a control.
            .pointingHand()
            .accessibilityLabel(editingRecipients ? "hide recipients" : "edit recipients")
            Text("·").foregroundStyle(Palette.inkFaintest)
            Text(replySubject)
                .font(Typo.micro)
                .foregroundStyle(Palette.inkFaintest)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 8)
            // The tracker switch is DESKTOP ONLY, same as the pane. This
            // header is one line in a reader column on a phone, already
            // carrying the recipients door and the subject; a switch nobody
            // came here to touch is what gets cut when that line has to hold
            // three things at phone width. The account default still decides.
            AttachButton(slot: .inlineReply)
            #if os(macOS)
                TrackerToggle(on: bindFlag(\.includeTracker))
            #endif
        }
    }

    /// Who the header names. A plain reply names the parent's sender; a
    /// reply-all names the fetched set — and until that lands (or when it never
    /// does) it says the derivation is pending rather than naming the sender,
    /// who is NOT certainly in the set: a mailing list's Reply-To routes the
    /// mail somewhere the sender's own address never appears.
    private func headerTarget(_ compose: ComposeState, parent: ClientMessage) -> String {
        // Once the fields hold the answer they ARE the answer, reply-all or
        // not: somebody who just moved a name to bcc has to see the header
        // agree with what they did.
        if compose.recipientsStated, let summary = recipientSummary(compose) { return summary }
        let sender = SenderCache.resolved(parent.senderString).displayName
        guard compose.replyAll else { return sender }
        return recipientsSettled ? "recipients derived at send" : "deriving recipients…"
    }

    /// "alice@example.com +3 more" — the header is one line above the mail, so a
    /// twelve-person thread has to collapse into a count rather than push the
    /// composer around. The full set is in the fields below, and in review.
    ///
    /// COUNTS BLIND COPIES IN THE TOTAL but never names one first: the summary
    /// leads with the visible audience, because "who is this to" is the question
    /// it answers. The bcc row states itself, in the fields and in review.
    private func recipientSummary(_ compose: ComposeState) -> String? {
        let r = compose.recipients
        let visible = r.tokens(.to) + r.tokens(.cc)
        let total = visible.count + r.count(.bcc)
        guard let first = visible.first ?? r.tokens(.bcc).first else { return nil }
        guard total > 1 else { return first }
        return "\(first) +\(total - 1) more"
    }

    /// THE THREE RECIPIENT FIELDS, opened from the header line.
    ///
    /// Not shown until the derivation has landed, and that is a correctness rule
    /// rather than a loading state: editing empty fields beforehand would make
    /// this composer state an audience it never learned, and on a reply-all the
    /// mail would go to one person instead of the room. The wait is one metadata
    /// fetch, and it started when the composer opened.
    @ViewBuilder
    private func recipientEditor(_ compose: ComposeState) -> some View {
        if compose.recipientsStated {
            RecipientFields(
                recipients: recipientsBinding, focus: $focusedField, field: { $0 },
                // The one field with nothing to seed it says why it is empty,
                // rather than reading as a value that got lost.
                placeholder: { $0 == .bcc ? "nobody is blind-copied" : nil })
                .padding(.bottom, 2)
        } else {
            Text("deriving recipients…")
                .font(Typo.micro)
                .foregroundStyle(Palette.inkFaintest)
        }
    }

    private func editor(_ compose: ComposeState) -> some View {
        // autofocus is the affordance: `r` must land the cursor in the body, or
        // the composer is a box you have to go click. It lives on the EDITOR,
        // not the bar: the editor also mounts on the way BACK from review (the
        // bar never left), so Esc out of review would otherwise drop the cursor
        // and hand every letter you typed next to the reader's verbs.
        MarkdownTextView(
            text: bind(\.body), autofocus: true, disabled: compose.sending,
            // Same two doors the pane's editor has, gated the same way.
            onDropFiles: store.composeAttachmentsAvailable
                ? { urls, at in ComposeAttach.add(urls: urls, to: .inlineReply, at: at) }
                : nil,
            onPasteImage: store.composeAttachmentsAvailable
                ? { png, at in
                    ComposeAttach.add(
                        data: png, filename: "pasted-image.png", mime: "image/png",
                        to: .inlineReply, at: at)
                } : nil,
            onDropHover: { dropTargeted = $0 }
        )
        .frame(height: Self.editorHeight)
        .padding(8)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(Palette.canvas.opacity(0.65))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .strokeBorder(
                    dropTargeted ? Palette.accent : Palette.hairlineStrong,
                    lineWidth: dropTargeted ? 1.5 : 0.75))
        .animation(.easeOut(duration: 0.12), value: dropTargeted)
    }

    /// The composer's own inset. The Mac's is the reader column's 22; a phone is
    /// narrower and the mail beside it is inset 18, so the reply lines up with
    /// the message it answers rather than sitting proud of it.
    #if os(macOS)
        private static let gutter: CGFloat = 22
        private static let editorHeight: CGFloat = 150
    #else
        private static let gutter: CGFloat = 18
        /// Short enough that a couple of lines of the email survive above a
        /// raised keyboard. The editor scrolls itself past that.
        private static let editorHeight: CGFloat = 104
    #endif

    #if os(macOS)
        private var hints: [KeyHint] {
            var hints = [KeyHint("⌘enter", "send")]
            if guarded { hints.append(KeyHint("⌘⇧enter", "send anyway")) }
            hints.append(KeyHint("esc", "dismiss"))
            return hints
        }
    #endif

    #if !os(macOS)
        /// THE PHONE'S HALF OF THE CEREMONY. The same `send(override:)`, the
        /// same rule that a blocked verdict is the ONLY thing that unlocks an
        /// override: what changes is that a thumb presses them instead of
        /// ⌘Enter and ⌘⇧Enter. The layout mirrors the pane composer's footer so
        /// a reply reads the same wherever it started.
        @ViewBuilder
        private func actionBar(_ compose: ComposeState) -> some View {
            HStack(spacing: 8) {
                Spacer()
                Button(ComposeLabels.dismiss) { store.closeInlineReply() }
                    .buttonStyle(.glass)
                if guarded {
                    Button("send anyway") { send(override: true) }
                        .buttonStyle(.glassProminent)
                        .tint(Palette.danger)
                }
                Button("send") { send(override: false) }
                    .buttonStyle(.glassProminent)
                    .tint(Palette.accent)
            }
            .padding(.horizontal, Self.gutter)
            .padding(.bottom, 10)
        }
    #endif

    // MARK: - keymap

    private var bindings: [KeyBinding] {
        [
            // Escape closes the composer, and only the NEXT press reaches the
            // viewer's Escape and closes the email. Plain Enter is not bound
            // at all: in the body it is a newline.
            KeyBinding("Escape", "dismiss reply", allowInInput: true) {
                store.closeInlineReply()
            },
            KeyBinding("Enter", "send", meta: true, allowInInput: true) {
                send(override: false)
            },
            // Explicit override: a blocked verdict, nothing else. Declines
            // otherwise.
            KeyBinding(declining: "shift+Enter", "send anyway", meta: true, allowInInput: true) {
                guard !store.askBarOpen, guarded else { return false }
                send(override: true)
                return true
            },
        ]
    }

    // MARK: - state helpers

    private func bind(_ keyPath: WritableKeyPath<ComposeState, String>) -> Binding<String> {
        Binding(
            get: { store.inlineReply?[keyPath: keyPath] ?? "" },
            set: { value in
                // The autosave's one hook for this composer — the body is the only
                // field it has, and it is bound through here.
                guard store.inlineReply?[keyPath: keyPath] != value else { return }
                patch { $0[keyPath: keyPath] = value }
                DraftSaver.shared.noteChange(.inlineReply)
            })
    }

    /// The three recipient fields as one binding. Writes go through
    /// `stateRecipients` — touching a recipient field is the sender taking the
    /// audience over from the daemon — and arm the autosave like any other
    /// edit: a bcc added and then abandoned has to come back.
    private var recipientsBinding: Binding<Recipients> {
        Binding(
            get: { store.inlineReply?.recipients ?? Recipients() },
            set: { value in
                guard store.inlineReply?.recipients != value else { return }
                patch { $0.stateRecipients(value) }
                DraftSaver.shared.noteChange(.inlineReply)
            })
    }

    /// Same shape as `bind`, minus the autosave: a draft records what was
    /// written, not how the next send is addressed.
    private func bindFlag(_ keyPath: WritableKeyPath<ComposeState, Bool>) -> Binding<Bool> {
        Binding(
            get: { store.inlineReply?[keyPath: keyPath] ?? false },
            set: { value in patch { $0[keyPath: keyPath] = value } })
    }

    private func patch(_ mutate: (inout ComposeState) -> Void) {
        guard var next = store.inlineReply else { return }
        mutate(&next)
        store.inlineReply = next
    }

    /// Same two checks as the pane composer, then the same hold.
    private func send(override: Bool) {
        guard let compose = store.inlineReply else { return }
        // Same seed rule as the pane: an untouched signature is an empty body.
        guard !Prefs.shared.isBodyUntouched(compose.body) else {
            patch { $0.error = "body is empty" }
            return
        }
        // Same tray rule as the pane: a file still uploading or one that
        // failed stops the send here, in words.
        if let problem = ComposeCopy.trayProblem(compose) {
            patch { $0.error = problem }
            return
        }
        store.sendWithUndo(.inlineReply, override: override)
    }
}
