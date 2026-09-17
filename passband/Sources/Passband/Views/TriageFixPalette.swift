// Explicit corrections for independent placement, kind, and agent access.
// The owner always keeps human access, including after an access restriction.

import SwiftUI

struct TriageFixPalette: View {
    let target: TriageFixTarget
    let onClose: () -> Void

    @Environment(AppStore.self) private var store
    @State private var query = ""
    @State private var selection = 0
    @State private var busy = false
    @State private var current: AgentMessageDecision?
    @Namespace private var paletteGlass
    @FocusState private var focused: Bool

    private var hits: [TriageTarget] { TriageTargets.match(query) }

    var body: some View {
        surface
            .keyContext(.modal)
            .keyBindings(.modal, bindings)
            .onAppear { focused = true }
            .task { current = try? await APIClient.shared.getAgentTriage(target.messageId).decision }
            .onChange(of: hits.count) { _, count in
                selection = max(0, min(selection, max(0, count - 1)))
            }
    }

    /// The palette itself. Same rows, same input, same apply — only the thing it
    /// is mounted IN differs: a command bar hanging under the Mac's chrome, or
    /// the phone's sheet, which brings its own scrim and its own dismissal.
    @ViewBuilder
    private var surface: some View {
        #if os(macOS)
            OverlayScrim(alignment: .top, topInset: 110, onDismiss: onClose) {
                GlassEffectContainer(spacing: 8) {
                    palette
                        .frame(width: 560)
                        .passbandGlass(
                            .pane, cornerRadius: 20, tint: Palette.glassTintStrong,
                            id: "palette", in: paletteGlass)
                        .shadow(color: .black.opacity(0.32), radius: 46, y: 20)
                }
            }
        #else
            palette
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                .background(Palette.canvas.ignoresSafeArea())
                // The keyboard is up the moment this opens — the field autofocuses
                // — so the list has to be able to get out from under it.
                .presentationDetents([.large])
        #endif
    }

    private var palette: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            wasRow
            input
            Divider().overlay(Palette.hairline)
            list
            footer
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "wand.and.sparkles")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Palette.accent)
            Text("Fix triage")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Palette.ink)
            Text(target.subject)
                .font(Typo.micro)
                .foregroundStyle(Palette.inkFaintest)
                .lineLimit(1)
                .help("\(target.sender) — \(target.subject)")
            Spacer(minLength: 4)
        }
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 8)
    }

    /// Display only the current authoritative decision, never legacy tiers.
    @ViewBuilder
    private var wasRow: some View {
        if let current {
            Text((current.kinds + current.destinations).joined(separator: " · ")
                .replacingOccurrences(of: "_", with: " "))
                .font(Typo.micro)
                .foregroundStyle(Palette.inkDim)
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
        }
    }

    private var input: some View {
        TextField("Change placement, kind, or agent access…", text: $query)
            .textFieldStyle(.plain)
            .font(.system(size: 15))
            .foregroundStyle(Palette.ink)
            .focused($focused)
            .autocorrectionDisabled()
            .disabled(busy)
            .padding(.horizontal, 16)
            .padding(.bottom, 12)
            .onChange(of: query) { _, _ in selection = 0 }
    }

    private var list: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 1) {
                    if hits.isEmpty {
                        Text(
                            "nothing matches “\(query)”. these are the only values the triage pipeline itself uses — anything else could not be learned from."
                        )
                        .font(Typo.micro)
                        .foregroundStyle(Palette.inkFaintest)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(14)
                    } else {
                        ForEach(Array(hits.enumerated()), id: \.element.id) { i, hit in
                            TargetRow(
                                target: hit, selected: i == selection,
                                onHover: { selection = i },
                                onPick: { Task { await apply(hit) } })
                            .id(hit.id)
                            .disabled(busy)
                        }
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
            }
            .frame(maxHeight: 300)
            .onChange(of: selection) { _, i in
                guard let hit = hits[safe: i] else { return }
                withAnimation(.easeOut(duration: 0.1)) { proxy.scrollTo(hit.id, anchor: .center) }
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 4) {
            // Three keycaps for three keys the phone does not have. There, the
            // rows are the pick, the tap is the apply, and pulling the sheet down
            // is the cancel — all of it already visible.
            #if os(macOS)
                Kbd("↑"); Kbd("↓")
                Text("pick").font(Typo.micro).foregroundStyle(Palette.inkFaintest)
                Text("·").foregroundStyle(Palette.inkFaintest)
                Kbd("↵")
                Text("apply").font(Typo.micro).foregroundStyle(Palette.inkFaintest)
                Text("·").foregroundStyle(Palette.inkFaintest)
                Kbd("esc")
                Text("cancel").font(Typo.micro).foregroundStyle(Palette.inkFaintest)
            #else
                Text("tap to apply").font(Typo.micro).foregroundStyle(Palette.inkFaintest)
            #endif
            Spacer()
            Text("stored to refine triage")
                .font(Typo.micro)
                .foregroundStyle(Palette.accent.opacity(0.8))
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .overlay(alignment: .top) { Hairline() }
    }

    /// allowInInput is REQUIRED, not polish: the palette autofocuses its field,
    /// so the `editing && !allowInInput` guard would drop every binding — and
    /// Escape would fall through to whatever binds it underneath, meaning
    /// cancelling would NAVIGATE the app.
    private var bindings: [KeyBinding] {
        [
            KeyBinding("Escape", "cancel", allowInInput: true) { onClose() },
            KeyBinding("ArrowDown", "next", allowInInput: true) {
                selection = min(hits.count - 1, selection + 1)
            },
            KeyBinding("ArrowUp", "prev", allowInInput: true) {
                selection = max(0, selection - 1)
            },
            KeyBinding("Enter", "apply", allowInInput: true) {
                if let hit = hits[safe: selection] { Task { await apply(hit) } }
            },
        ]
    }

    /// Corrections preserve unrelated dimensions; refreshed server projections
    /// decide which visible rows change after the write succeeds.
    private func apply(_ hit: TriageTarget) async {
        guard !busy else { return }
        busy = true
        do {
            try await APIClient.shared.correctTriage(
                messageId: target.messageId, target: hit)
        } catch {
            store.pushToast(errText(error, "could not record the correction"), .error)
            busy = false
            return
        }
        Analytics.capture("triage_corrected", ["axis": hit.axis.rawValue, "to": hit.value])
        // A restriction changes external access only. The owner keeps the
        // message and its placements. Refresh authoritative projections instead
        // of inferring a move from its kind or access assessment.
        ThreadPrefetch.shared.wipe()
        store.pushToast("\(hit.label) · recorded", .success)
        onClose()
        // Runs on past the dismissal on purpose: the calling Task belongs to the
        // key binding, not to this view, so closing does not cancel it.
        await SitrepPoller.shared.refreshAfterCorrection()
    }
}

private struct TargetRow: View {
    let target: TriageTarget
    let selected: Bool
    let onHover: () -> Void
    let onPick: () -> Void

    private var axisTone: Color {
        switch target.axis {
        case .showInFye: Palette.warn
        case .kinds, .destinations: Palette.accent
        case .externalAccess: Palette.lock
        }
    }

    var body: some View {
        // hoverFill off: the pointer MOVES the selection here, so the selection
        // fill is already following it — a hover wash under it would double up.
        ListRow(
            selected: selected, tint: axisTone, hPadding: 10, vPadding: 7,
            hoverFill: false, onHoverChange: { if $0 { onHover() } }, action: onPick
        ) { _, _ in
            HStack(spacing: 10) {
                Chip(text: target.axis.chipLabel, tone: axisTone, filled: true)
                    .frame(width: 68, alignment: .leading)
                Text(target.label)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(Palette.ink)
                    .frame(width: 130, alignment: .leading)
                Text(target.hint)
                    .font(Typo.micro)
                    .foregroundStyle(Palette.inkFaint)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)

            }
        }
    }
}
