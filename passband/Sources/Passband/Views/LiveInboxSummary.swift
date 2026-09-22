import SwiftUI

/// The only post-practice modal: observed counts from the connected mailbox.
/// It mounts the moment the live shell is back, so the first thing it shows
/// may be the wait for that mailbox's first update.
struct LiveInboxSummary: View {
    @Environment(AppStore.self) private var store
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var appeared = false

    private var tour: TourController { store.tour }

    var body: some View {
        OverlayScrim(onDismiss: { tour.finish() }) {
            ModalCard(width: 640) {
                VStack(alignment: .leading, spacing: 22) {
                    Label("PASSBAND", systemImage: "sparkles")
                        .font(Typo.sectionLabel)
                        .tracking(2)
                        .foregroundStyle(Palette.accentInk)
                    Text("Your inbox is taking shape.")
                        .font(Typo.serif(32, weight: .medium))
                        .foregroundStyle(Palette.ink)
                    Text("See what needs your attention. Everything else has a place.")
                        .font(.system(size: 15))
                        .foregroundStyle(Palette.inkDim)
                    if let stats = store.sitrep.stats {
                        HStack(spacing: 10) {
                            tile(stats.total, "emails triaged", "tray", Palette.accent)
                            tile(stats.tier_counts["noise"] ?? 0, "noise set aside", "moon", Palette.positive)
                            tile(stats.bands.standing, "need your attention", "eye", Palette.accentInk)
                        }
                        .animation(reduceMotion ? nil : .easeOut(duration: 0.35), value: stats)
                    } else {
                        ProgressView("Waiting for your mailbox’s first update…")
                    }
                    Label(progress, systemImage: store.daemonDown ? "wifi.exclamationmark" : "arrow.triangle.2.circlepath")
                        .font(.system(size: 13))
                        .foregroundStyle(Palette.inkDim)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 14) {
                        Button("Go to my inbox") { tour.finish() }
                            .buttonStyle(.borderedProminent)
                        KeyHint("enter", "let’s begin")
                    }
                }
                .padding(14)
            }
            .opacity(appeared ? 1 : 0)
            .scaleEffect(appeared || reduceMotion ? 1 : 0.96)
            .offset(y: appeared || reduceMotion ? 0 : 12)
        }
        .onAppear {
            withAnimation(reduceMotion ? nil : .easeOut(duration: 0.45)) { appeared = true }
        }
        .keyContext(.modal)
        .keyBindings(.modal, [
            KeyBinding("Enter", "go to my inbox") { tour.finish() },
            KeyBinding("Escape", "close inbox summary") { tour.finish() },
        ])
    }

    private var progress: String {
        if store.daemonDown { return "Connection interrupted. Your inbox will update when we reconnect." }
        if store.sitrep.stats?.gmail?.connected == false {
            return "Your mail connection needs attention in Settings before new mail can arrive."
        }
        if let catchUp = store.sitrep.stats?.catch_up {
            return "Catching up · \(catchUp.done.formatted()) of \(catchUp.total.formatted()) emails fetched. Triage continues as your mailbox catches up."
        }
        return "These are your live mailbox counts. You can start now; new results appear as your inbox updates."
    }

    private func tile(_ count: Int, _ label: String, _ symbol: String, _ color: Color) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Image(systemName: symbol).foregroundStyle(color)
            Text(count.formatted())
                .font(Typo.num(32, weight: .semibold))
                .contentTransition(.numericText())
            Text(label).font(.system(size: 12)).foregroundStyle(Palette.inkDim)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(color.opacity(0.07), in: RoundedRectangle(cornerRadius: 14))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(count) \(label)")
    }
}
