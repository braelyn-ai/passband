import AppKit
import Observation
import SwiftUI

/// The tester's side of a standalone rehearsal (`--onboarding-rehearsal`):
/// the opening beats ahead of the practice inbox, and Start fresh. Customer
/// onboarding never shows the intro here — theirs is the Connect gate's —
/// and never opens the panel.
@MainActor
@Observable
final class RehearsalSession {
    static let shared = RehearsalSession()
    var showingIntro = RehearsalMode.launchedStandalone && !RehearsalMode.includesConnection

    private(set) var runID = UUID()
    private(set) var mailboxReady = false
    private(set) var entryRequested = false

    /// Fill the actual practice store while the opening remains on screen.
    func prepareMailbox() async {
        guard showingIntro else { return }
        let run = runID
        let tour = AppStore.shared.tour
        tour.replay(store: AppStore.shared, source: .rehearsal, preloading: true)
        await tour.waitForPracticePreparation()
        guard !Task.isCancelled, runID == run, tour.active else { return }
        mailboxReady = true
        if entryRequested { showingIntro = false }
    }

    func enterMailbox() {
        entryRequested = true
        if mailboxReady { showingIntro = false }
    }

    func startFresh() {
        if RehearsalMode.includesConnection {
            Task { await restartConnection() }
            return
        }
        AppStore.shared.tour.cancel()
        mailboxReady = false
        entryRequested = false
        runID = UUID()
        showingIntro = true
        MainWindow.show()
    }

    private(set) var restartingConnection = false

    func restartConnection() async {
        guard !restartingConnection else { return }
        restartingConnection = true
        defer { restartingConnection = false }
        let store = AppStore.shared
        store.tour.cancel()
        await store.resetForConnectionRehearsal()
        store.tour.prepareConnectionRehearsal()
        runID = UUID()
    }

}

@MainActor
final class RehearsalControlPanel {
    static let shared = RehearsalControlPanel()
    private var panel: NSPanel?

    func show() {
        guard RehearsalMode.launchedStandalone else { return }
        if let panel { panel.orderFront(nil); return }
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 360, height: RehearsalMode.includesConnection ? 250 : 172),
            styleMask: [.titled, .closable, .utilityWindow],
            backing: .buffered, defer: false)
        panel.title = "Onboarding rehearsal"
        panel.identifier = NSUserInterfaceItemIdentifier("rehearsal-controls")
        panel.isReleasedWhenClosed = false
        panel.hidesOnDeactivate = false
        panel.level = .normal
        panel.contentView = NSHostingView(rootView: RehearsalControls())
        if let screen = MainWindow.find()?.screen ?? NSScreen.main {
            let bounds = screen.visibleFrame
            panel.setFrameTopLeftPoint(NSPoint(x: bounds.maxX - 380, y: bounds.maxY - 28))
        } else { panel.center() }
        self.panel = panel
        panel.orderFront(nil)
    }
}

private struct RehearsalControls: View {
    @State private var rehearsal = RehearsalSession.shared
    @State private var prefs = Prefs.shared
    @State private var store = AppStore.shared
    @State private var linkMessage: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Onboarding rehearsal")
                .font(.system(size: 15, weight: .semibold))
            Text("The main window shows the customer experience. Appearance is the app's real setting; Start fresh returns to the opening and reloads the practice mail.")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 12) {
                Button(prefs.theme == .dark ? "Light appearance" : "Dark appearance") { prefs.flipTheme() }
                Button("Start fresh") { rehearsal.startFresh() }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(store.tour.veil != nil || rehearsal.restartingConnection)
            if RehearsalMode.includesConnection {
                Button("Use copied login link") {
                    guard let text = NSPasteboard.general.string(forType: .string),
                          let url = URL(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
                          PairLink(url) != nil else {
                        linkMessage = "Copy the Passband login link from your browser first."
                        return
                    }
                    store.receivePairLink(url)
                    linkMessage = "Login link opened in this rehearsal."
                    MainWindow.show()
                }
                .disabled(store.accountActionsBlocked || rehearsal.restartingConnection)
                .help("Open the copied login link here if macOS sends it to an older Passband build.")
                if let linkMessage {
                    Text(linkMessage).font(.system(size: 11)).foregroundStyle(.secondary)
                }
            }
        }
        .padding(20)
        .frame(width: 360, alignment: .leading)
        .preferredColorScheme(prefs.theme.colorScheme)
    }
}
