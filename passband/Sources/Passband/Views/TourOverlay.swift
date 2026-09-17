import SwiftUI

/// The coordinate space every measured region and the overlay itself report
/// into. Named on MainShell's outer ZStack, which is the common ancestor of the
/// sitrep and this overlay.
let tourSpace = "passband-tour"

extension View {
    /// Tag a dashboard region so a coach mark can ring it. Safe to leave on a
    /// zone permanently: outside a running tour the measurement lands in a
    /// non-observed shadow copy and invalidates nothing (see
    /// `TourController.report`).
    func tourTarget(_ id: TourTarget) -> some View {
        modifier(TourTargetModifier(id: id))
    }
}

private struct TourTargetModifier: ViewModifier {
    @Environment(AppStore.self) private var store
    let id: TourTarget

    func body(content: Content) -> some View {
        content
            .onGeometryChange(for: CGRect.self) {
                $0.frame(in: .named(tourSpace))
            } action: { store.tour.report(id, $0) }
    }
}

/// The guide over the practice board, then the summary over the live one.
/// The phone has no practice inbox yet (its guide is still to be designed),
/// so its controller only ever reaches the summary.
struct TourOverlay: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        switch store.tour.phase {
        case .practice:
            #if os(macOS)
                if store.tour.practiceStep == .welcome || store.tour.practiceStep == .wrap {
                    PracticeBoundaryModal()
                } else {
                    PracticeProductTour()
                }
            #else
                EmptyView()
            #endif
        case .summary:
            LiveInboxSummary()
        }
    }
}

#if os(macOS)
/// The board is already loaded behind this card. Continuing only removes
/// the blur and begins the lessons; it never fetches or replaces the mailbox.
private struct PracticeBoundaryModal: View {
    @Environment(AppStore.self) private var store

    private var closing: Bool { store.tour.practiceStep == .wrap }

    var body: some View {
        OverlayScrim(onDismiss: {}) {
            ModalCard(width: 580) {
                VStack(alignment: .leading, spacing: 22) {
                    Label("PASSBAND GUIDE", systemImage: "sparkles")
                        .font(Typo.sectionLabel)
                        .tracking(1)
                        .foregroundStyle(Palette.accentInk)
                    Text(closing ? "A little less to carry." : "A little practice.")
                        .font(Typo.serif(34, weight: .medium))
                        .foregroundStyle(Palette.ink)
                    Text(closing
                         ? "You know your way around. Let’s open your inbox and see what needs your attention."
                         : "Passband is an entirely new way of reading email. With a little practice, you'll be an expert. Here's a quick guide!")
                        .font(.system(size: 18))
                        .lineSpacing(5)
                        .foregroundStyle(Palette.inkDim)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 18) {
                        if closing {
                            Button("Back") { store.tour.back() }
                                .buttonStyle(.plain)
                                .foregroundStyle(Palette.inkFaint)
                        }
                        Button(closing ? "Open my inbox" : "Show me around", action: proceed)
                            .buttonStyle(.borderedProminent)
                            .tint(Palette.accent)
                            .controlSize(.large)
                            .font(.system(size: 15, weight: .semibold))
                        if !closing {
                            // The way out from the very first card: nobody
                            // should have to take one lesson to decline them.
                            Button("Skip the guide") { store.tour.skip() }
                                .buttonStyle(.plain)
                                .foregroundStyle(Palette.inkFaint)
                                .help("Go straight to your inbox · Esc")
                        }
                    }
                    .disabled(store.tour.veil != nil)
                }
                .padding(14)
            }
        }
        .keyContext(.modal)
        .keyBindings(.modal, [
            KeyBinding("Enter", "continue onboarding") { proceed() },
            KeyBinding("Escape", closing ? "open my inbox" : "skip the guide") {
                guard store.tour.veil == nil else { return }
                if closing { store.tour.completePractice() } else { store.tour.skip() }
            },
        ])
    }

    private func proceed() {
        guard store.tour.veil == nil else { return }
        if closing { store.tour.completePractice() }
        else if store.tour.practiceStep == .welcome { store.tour.advancePractice() }
    }
}
#endif
