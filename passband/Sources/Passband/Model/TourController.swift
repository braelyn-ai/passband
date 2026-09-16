// The guided practice inbox, then the live mailbox's summary — both inside
// the one app. Practice is a MODE of the store (`AppStore.enterPractice`):
// the same window, the same shell, fixture mail behind the same transport.
// Leaving it swaps the live account back in under a veil and the summary
// card reads the first real counts as they land.
import Foundation
import Observation
import SwiftUI

enum TourTarget: Hashable, Sendable {
    case records, newsletters, eyes, calendar, shipments, banking, receipts, brightly, maya
}

/// Which card the overlay shows. `practice` is the guide over fixture mail;
/// `summary` is the one modal over the live board that ends onboarding.
enum TourPhase: Sendable { case practice, summary }

/// Where a run came from, for the funnel. `rehearsal` never leaves the
/// machine (a standalone launch has no analytics client).
enum TourSource: String, Sendable { case firstRun = "first_run", settings, rehearsal }

@MainActor
@Observable
final class TourController {
    static let forced = ProcessInfo.processInfo.environment["PASSBAND_FORCE_TOUR"] == "1"
    static let practiceRuleSender = "updates@brightly.example"
    private(set) var active = false
    private(set) var preparingPractice = false
    private(set) var phase: TourPhase = .practice
    /// While non-nil the shell is veiled behind this line: the practice
    /// inbox is being set up, or the live one is being put back.
    private(set) var veil: String?
    private(set) var practiceStep: PracticeTourStep = .welcome {
        didSet { practiceRecordThreadID = nil }
    }
    private(set) var practiceRecordThreadID: String?
    private(set) var practiceRunID = UUID()
    private(set) var practiceDoneID: UUID?
    private(set) var practiceRuleSaved = false
    private(set) var practiceRuleMuted = false
    /// Session-only: reconnecting after practice must not repeat the intro.
    private(set) var hasLeftPractice = false
    private var summaryAfterConnection = false
    private var rehearsalConnectionPending = RehearsalMode.includesConnection
    private(set) var targets: [TourTarget: CGRect] = [:]
    @ObservationIgnored private var measured: [TourTarget: CGRect] = [:]
    @ObservationIgnored private var replayTask: Task<Void, Never>?
    @ObservationIgnored private var replayGeneration = UUID()
    @ObservationIgnored private var leaving: Task<Void, Never>?
    private var dismissedThisSession = false
    private var source: TourSource = .firstRun

    /// The welcome reveals the practice board; subsequent lessons point at it.
    var wantsBlur: Bool { active && (phase == .summary || practiceStep == .welcome || practiceStep == .wrap) }

    /// Gate the first connected frame, before SwiftUI runs connection observers.
    var blocksMailboxForPractice: Bool {
        preparingPractice || (!active && !summaryAfterConnection &&
            (rehearsalConnectionPending ||
                (!dismissedThisSession && (!Prefs.shared.tourCompleted || Self.forced))))
    }

    func prepareConnectionRehearsal() {
        cancel()
        hasLeftPractice = false
        summaryAfterConnection = false
        rehearsalConnectionPending = true
    }

    /// Start at connection, before live mail is rendered or fetched.
    func maybeStart() {
        let store = AppStore.shared
        guard !active, !preparingPractice, store.connStatus == .connected else { return }
        if rehearsalConnectionPending {
            rehearsalConnectionPending = false
            replay(store: store, source: .rehearsal)
            return
        }
        if summaryAfterConnection {
            showLiveSummary()
            return
        }
        guard !dismissedThisSession, !Prefs.shared.tourCompleted || Self.forced else { return }
        replay(store: AppStore.shared, source: .firstRun)
    }

    /// Settings' "try the practice inbox", the first-run trigger and the
    /// tester's Start fresh all come through here. On the phone there is no
    /// practice inbox yet, so the run is the summary alone.
    func replay(store: AppStore, source: TourSource = .settings, preloading: Bool = false) {
        cancelReplay()
        dismissedThisSession = false
        self.source = source
        summaryAfterConnection = false
        store.setView(.sitrep)
        #if os(macOS)
            preparingPractice = true
            if !preloading { veil = "Setting up your practice inbox…" }
            let generation = replayGeneration
            replayTask = Task { @MainActor in
                // A cancel that lands before this body runs must not wipe the
                // live board for a practice nobody is going to see.
                guard !Task.isCancelled, self.replayGeneration == generation else { return }
                await store.enterPractice()
                guard !Task.isCancelled, self.replayGeneration == generation else {
                    // Cancelled after the swap: the store is in practice mode
                    // with no guide to leave it by. Unless a newer replay is
                    // already queued to take the fixtures over, put the live
                    // account back rather than strand a board nobody can exit.
                    if RehearsalMode.isEnabled, !self.active, self.replayTask == nil {
                        await store.exitPractice()
                    }
                    return
                }
                self.startPractice()
                await store.warmPractice()
                guard !Task.isCancelled, self.replayGeneration == generation else { return }
                self.preparingPractice = false
                self.veil = nil
                Analytics.capture("tour_started", ["source": source.rawValue])
            }
        #else
            showLiveSummary()
        #endif
    }

    /// The intro joins the existing preparation instead of starting it again.
    func waitForPracticePreparation() async {
        await replayTask?.value
    }

    private func cancelReplay() {
        replayTask?.cancel()
        replayTask = nil
        replayGeneration = UUID()
        preparingPractice = false
        veil = nil
    }

    private func startPractice() {
        practiceStep = .welcome
        practiceRunID = UUID()
        practiceDoneID = nil
        practiceRuleSaved = false
        practiceRuleMuted = false
        targets = measured
        phase = .practice
        active = true
    }

    func showLiveSummary() {
        cancelReplay()
        summaryAfterConnection = false
        phase = .summary
        active = true
        AppStore.shared.setView(.sitrep)
    }

    func back() {
        guard let previous = PracticeTourStep(rawValue: practiceStep.rawValue - 1) else { return }
        practiceStep = previous
    }

    /// The wrap card's button: the guide was seen through to its end.
    func completePractice() {
        guard active, phase == .practice, leaving == nil else { return }
        Analytics.capture("tour_completed", ["step": practiceStep.rawValue + 1])
        exploreInbox()
    }

    /// Skip during practice leaves for the live inbox; skip on the summary
    /// is the same as taking it. A press while the veil is already up counts
    /// nothing twice.
    func skip() {
        guard active else { return }
        if phase == .practice {
            guard leaving == nil else { return }
            Analytics.capture("tour_skipped", ["step": practiceStep.rawValue + 1])
            exploreInbox()
        } else {
            finish()
        }
    }

    /// Leave practice for the live inbox. Idempotent: a second press while
    /// the veil is up joins the departure already under way.
    func exploreInbox() {
        guard RehearsalMode.isEnabled, active, phase == .practice, leaving == nil else { return }
        // Veiled on the press itself, not a tick later: the button has to
        // read as having done something before the departure's first await.
        veil = "Opening your inbox…"
        leaving = Task { @MainActor in
            await self.leavePractice()
            self.leaving = nil
        }
    }

    private func leavePractice() async {
        let store = AppStore.shared
        // Long enough for the zoom-out to read as leaving somewhere.
        try? await Task.sleep(for: .milliseconds(650))
        releaseHeldUndo()
        // The summary is what mounts when the live shell comes back, so it
        // is the phase BEFORE the swap: a frame of the guide over a board it
        // cannot find would otherwise open Maya's reader on the real mail.
        phase = .summary
        hasLeftPractice = true
        await store.exitPractice()
        if store.connStatus == .connected {
            active = true
            store.setView(.sitrep)
        } else {
            // If the account cannot be restored, connect directly without
            // repeating the intro. Continue with the summary once connected.
            cancel()
            summaryAfterConnection = true
        }
        veil = nil
    }

    /// Start fresh and view teardown: drop the guide without ending
    /// onboarding. Practice itself is the store's to leave.
    func cancel() {
        cancelReplay()
        releaseHeldUndo()
        active = false
        phase = .practice
        practiceRecordThreadID = nil
        dismissedThisSession = true
    }

    func finish() {
        cancel()
        Prefs.shared.tourCompleted = true
    }

    func advancePractice() {
        guard active, RehearsalMode.isEnabled else { return }
        guard let next = PracticeTourStep(rawValue: practiceStep.rawValue + 1) else {
            completePractice()
            return
        }
        practiceStep = next
    }

    /// Skipping an interaction skips its dependent lesson too, without
    /// displaying a success that did not happen. The done lesson's undo chip
    /// waits for the learner (see `AppStore.pushUndo`), so skipping past it
    /// is what retires the chip.
    func skipPracticeStep() {
        guard active, RehearsalMode.isEnabled else { return }
        switch practiceStep {
        case .openMaya, .done, .undo:
            releaseHeldUndo()
            practiceStep = .calendar
        case .openBrightly, .rule: practiceStep = .wrap
        default: advancePractice()
        }
    }

    /// The undo entry appears only after the actual Done API call succeeds.
    /// A second done (Back to the lesson, `e` again) replaces the chip rather
    /// than stacking a second immortal one beside it.
    func notePracticeDone(_ undoID: UUID) {
        guard active, RehearsalMode.isEnabled, practiceStep == .done else { return }
        if let held = practiceDoneID, held != undoID {
            AppStore.shared.undos.removeAll { $0.id == held }
        }
        practiceDoneID = undoID
    }

    private func releaseHeldUndo() {
        guard let held = practiceDoneID else { return }
        AppStore.shared.undos.removeAll { $0.id == held }
        practiceDoneID = nil
    }

    func notePracticeRuleSaved(run: UUID, muted: Bool = false) {
        guard active, RehearsalMode.isEnabled, practiceRunID == run,
            practiceStep == .rule else { return }
        practiceRuleSaved = true
        practiceRuleMuted = muted
        advancePractice()
    }

    /// Reading any example in the highlighted category arms the return
    /// lesson. Only closing that loaded reader completes it; unrelated mail,
    /// failed loads, and callbacks from a prior run cannot advance the tour.
    func observePracticeRecordReader(
        threadID: String?, loadedThreadID: String?, category: TourTarget?, run: UUID
    ) {
        guard active, RehearsalMode.isEnabled, practiceRunID == run,
              let expected = practiceStep.recordCategory else { return }
        guard let threadID else {
            if practiceRecordThreadID != nil { advancePractice() }
            return
        }
        if practiceRecordThreadID != threadID { practiceRecordThreadID = nil }
        if loadedThreadID == threadID, category == expected {
            practiceRecordThreadID = threadID
        }
    }

    // Geometry follows the real dashboard in both window sizes.
    func report(_ id: TourTarget, _ rect: CGRect) {
        measured[id] = rect
        guard active, targets[id] != rect else { return }
        targets[id] = rect
    }
}
