import Foundation

// Narrow collaborators for the real controller. No app, preferences domain,
// keychain or network is created by this suite; the store stub flips the
// real `RehearsalMode` flag exactly where the real store does.
@MainActor
final class Prefs {
    static let shared = Prefs()
    var tourCompleted = false
}
@MainActor
final class AppStore {
    enum View { case sitrep, settings }
    enum Connection { case loading, connected, disconnected }
    struct Undo { let id: UUID }
    static let shared = AppStore()
    var activeView: View = .sitrep
    var connStatus: Connection = .connected
    var daemonDown = false
    var undos: [Undo] = []
    var enters = 0
    var warms = 0
    var exits = 0
    /// What the live account comes back as after practice.
    var connectionAfterExit: Connection = .connected
    func setView(_ view: View) { activeView = view }
    /// Shaped like the real one: a `.loading` frame, a suspension point
    /// (draft settlement, the stats read), then the swap — and, like the
    /// real one past its abort point, deliberately deaf to cancellation, so
    /// the controller's handling of a cancel that lands mid-swap is tested.
    func enterPractice() async {
        enters += 1
        connStatus = .loading
        try? await Task.sleep(for: .milliseconds(120))
        RehearsalMode.setEnabled(true)
        connStatus = .connected
    }
    func warmPractice() async { warms += 1 }
    func exitPractice() async {
        exits += 1
        RehearsalMode.setEnabled(false)
        connStatus = connectionAfterExit
    }
}
enum Analytics {
    nonisolated(unsafe) static var events: [(String, [String: Any])] = []
    static func capture(_ event: String, _ properties: [String: Any] = [:]) {
        events.append((event, properties))
    }
}

@main
struct TourControllerTests {
    @MainActor
    /// Longer than the controller's veil beats (450ms in, 650ms out).
    static func settle() async throws { try await Task.sleep(for: .milliseconds(950)) }

    @MainActor
    static func main() async throws {
        precondition(!RehearsalMode.isEnabled, "The suite starts in the live app")
        if RehearsalMode.includesConnection {
            try await connectionRehearsal()
            print("Connection rehearsal tests passed")
            return
        }
        try await preload()
        try await firstRun()
        try await lessons()
        try await recordLessons()
        try await standaloneWithoutAccount()
        print("Tour controller tests passed")
    }

    @MainActor
    static func connectionRehearsal() async throws {
        let store = AppStore.shared
        Prefs.shared.tourCompleted = true
        store.connStatus = .disconnected
        let tour = TourController()
        tour.prepareConnectionRehearsal()
        tour.maybeStart()
        precondition(!RehearsalMode.isEnabled && store.enters == 0)
        store.connStatus = .connected
        tour.maybeStart()
        try await settle()
        precondition(tour.active && tour.phase == .practice && store.enters == 1,
                     "Connection rehearsal enters practice even on a previously onboarded install")
        tour.skip()
        try await settle()
        precondition(tour.phase == .summary && !RehearsalMode.isEnabled)
        tour.finish()
        tour.maybeStart()
        try await settle()
        precondition(store.enters == 1, "Rehearsed connection does not loop back into practice")
    }

    @MainActor
    static func preload() async throws {
        let store = AppStore.shared
        let tour = TourController()
        tour.replay(store: store, source: .rehearsal, preloading: true)
        await tour.waitForPracticePreparation()
        precondition(tour.active && tour.practiceStep == .welcome && tour.wantsBlur)
        precondition(tour.veil == nil, "Preloading must not replace the intro with a loading screen")
        precondition(store.enters == 1 && store.warms == 1, "Preparation finishes before the intro reveals the board")
        await tour.waitForPracticePreparation()
        precondition(store.enters == 1 && store.warms == 1, "Continue joins preparation without resetting the mailbox")
        tour.cancel()
        RehearsalMode.setEnabled(false)
        store.enters = 0
        store.warms = 0
    }

    @MainActor
    static func firstRun() async throws {
        let store = AppStore.shared
        let cancelled = TourController()
        cancelled.replay(store: store)
        cancelled.cancel()
        try await settle()
        precondition(store.enters == 0, "Cancel before replay executes must not enter practice")

        let midFlight = TourController()
        midFlight.replay(store: store)
        try await Task.sleep(for: .milliseconds(40))
        precondition(store.enters == 1 && !RehearsalMode.isEnabled, "The swap is still ahead")
        midFlight.cancel()
        try await settle()
        precondition(!RehearsalMode.isEnabled && store.exits == 1 && !midFlight.active,
                     "A cancel that lands after the swap puts the live account back instead of stranding fixture mail")
        store.enters = 0
        store.exits = 0

        let tour = TourController()
        precondition(tour.blocksMailboxForPractice, "First connected frame must hide live mail")
        tour.maybeStart()
        precondition(tour.preparingPractice && tour.blocksMailboxForPractice,
                     "Preparation gates rendering synchronously, before its task runs")
        tour.maybeStart()
        try await settle()
        precondition(store.enters == 1 && store.warms == 1, "Repeated first-run checks enter practice once")
        precondition(tour.active && tour.phase == .practice && tour.veil == nil)
        precondition(!tour.blocksMailboxForPractice && tour.wantsBlur,
                     "Prepared mailbox is revealed with its welcome modal already active")
        precondition(RehearsalMode.isEnabled, "Practice is a mode of the one store")
        precondition(Analytics.events.last?.0 == "tour_started")
        precondition(Analytics.events.last?.1["source"] as? String == "first_run")
        tour.maybeStart()
        try await settle()
        precondition(store.enters == 1, "A running tour is not restarted by a later first sync")

        tour.skip()
        precondition(tour.veil != nil && tour.active, "Leaving is veiled, not torn down")
        precondition(Analytics.events.last?.0 == "tour_skipped")
        precondition(Analytics.events.last?.1["step"] as? Int == 1)
        let skips = Analytics.events.count
        tour.skip()
        precondition(Analytics.events.count == skips, "A second skip under the veil counts nothing twice")
        try await settle()
        precondition(store.exits == 1, "A second skip joins the departure under way")
        precondition(!RehearsalMode.isEnabled && tour.phase == .summary && tour.active && tour.veil == nil,
                     "Skip leaves practice for the live summary in the same process")
        precondition(!Prefs.shared.tourCompleted, "The summary is still onboarding")
        tour.skip()
        precondition(Prefs.shared.tourCompleted && !tour.active, "Skip on the summary takes it")
        tour.maybeStart()
        try await settle()
        precondition(store.enters == 1, "Completed onboarding does not restart itself")
    }

    @MainActor
    static func lessons() async throws {
        let store = AppStore.shared
        Prefs.shared.tourCompleted = false
        let tour = TourController()
        tour.replay(store: store)
        try await settle()
        precondition(tour.active && tour.practiceStep == .welcome)
        precondition(tour.wantsBlur, "Welcome presents over the blurred practice board")
        tour.advancePractice()
        precondition(tour.practiceStep == .needsYou && !tour.wantsBlur,
                     "Welcome confirmation reveals the board and begins its first lesson")
        precondition(Analytics.events.last?.1["source"] as? String == "settings")

        while tour.practiceStep != .done { tour.advancePractice() }
        let held = UUID(), unrelated = UUID()
        store.undos = [.init(id: held), .init(id: unrelated)]
        tour.notePracticeDone(held)
        let again = UUID()
        store.undos.append(.init(id: again))
        tour.notePracticeDone(again)
        precondition(store.undos.map(\.id) == [unrelated, again], "A second done replaces the held chip")
        tour.skipPracticeStep()
        precondition(tour.practiceStep == .calendar && store.undos.map(\.id) == [unrelated],
                     "Skipping past the undo lesson retires the chip it was waiting on")

        tour.back(); tour.back()
        precondition(tour.practiceStep == .done)
        store.undos.append(.init(id: held))
        tour.notePracticeDone(held)
        tour.cancel()
        precondition(store.undos.map(\.id) == [unrelated], "Teardown removes only the undo held by its lesson")
        precondition(!tour.active && RehearsalMode.isEnabled, "Cancel drops the guide; practice is the store's to leave")
        precondition(!Prefs.shared.tourCompleted, "View teardown is not tour completion")
        RehearsalMode.setEnabled(false)

        tour.replay(store: store)
        try await settle()
        let oldRun = tour.practiceRunID
        tour.cancel()
        tour.replay(store: store)
        try await settle()
        while tour.practiceStep != .rule { tour.advancePractice() }
        tour.notePracticeRuleSaved(run: oldRun)
        precondition(tour.practiceStep == .rule && !tour.practiceRuleSaved, "A stale editor callback cannot advance a new run")
        tour.notePracticeRuleSaved(run: tour.practiceRunID, muted: true)
        precondition(tour.practiceStep == .ruleSaved && tour.practiceRuleSaved && tour.practiceRuleMuted,
                     "A verified mute gets its own confirmation")
        tour.advancePractice()
        precondition(tour.practiceStep == .wrap && tour.veil == nil && tour.wantsBlur,
                     "The closing practice modal mirrors the blurred welcome")
        let exitsBefore = store.exits
        tour.advancePractice()
        precondition(Analytics.events.last?.0 == "tour_completed")
        precondition(Analytics.events.last?.1["step"] as? Int == PracticeTourStep.allCases.count)
        try await settle()
        precondition(store.exits == exitsBefore + 1 && tour.phase == .summary && tour.active)
        tour.finish()
        precondition(!tour.active && Prefs.shared.tourCompleted)

        tour.replay(store: store)
        try await settle()
        precondition(!tour.practiceRuleMuted, "A new run cannot inherit an earlier mute promise")
        while tour.practiceStep != .rule { tour.advancePractice() }
        tour.notePracticeRuleSaved(run: tour.practiceRunID, muted: false)
        precondition(tour.practiceStep == .ruleSaved && tour.practiceRuleSaved && !tour.practiceRuleMuted,
                     "A different saved disposition confirms saving without promising the sender is hidden")
        tour.cancel()
        RehearsalMode.setEnabled(false)
        precondition(!PracticeTourStep.shipments.isInteraction, "Shipments offer normal Next without requiring an open")
        precondition(PracticeTourStep.openMaya.targets == [.maya], "The read lesson highlights only Maya’s row")
        precondition(PracticeTourStep.openBrightly.targets == [.brightly], "The terms lesson highlights only Brightly’s row")
    }

    @MainActor
    static func recordLessons() async throws {
        let tour = TourController()
        tour.replay(store: AppStore.shared)
        try await settle()
        while tour.practiceStep != .calendar { tour.advancePractice() }
        let run = tour.practiceRunID

        func observe(_ thread: String?, loaded: String?, category: TourTarget?, token: UUID? = nil) {
            tour.observePracticeRecordReader(threadID: thread, loadedThreadID: loaded,
                                             category: category, run: token ?? run)
        }

        observe("practice-10", loaded: nil, category: .calendar)
        observe(nil, loaded: nil, category: nil)
        precondition(tour.practiceStep == .calendar, "Closing an unloaded reader does not complete a lesson")
        observe("practice-5", loaded: "practice-5", category: .shipments)
        observe(nil, loaded: nil, category: nil)
        precondition(tour.practiceStep == .calendar, "Opening and closing another category cannot complete Calendar")
        observe("practice-17", loaded: "practice-17", category: .calendar, token: UUID())
        precondition(tour.practiceRecordThreadID == nil, "A stale run cannot arm a reader return")

        observe("practice-10", loaded: "practice-10", category: .calendar)
        precondition(tour.practiceRecordThreadID == "practice-10", "Any loaded calendar row teaches Escape")
        observe("practice-10", loaded: "practice-10", category: .calendar)
        precondition(tour.practiceStep == .calendar, "Repeated load notifications do not advance")
        observe(nil, loaded: nil, category: nil)
        precondition(tour.practiceStep == .shipments && tour.practiceRecordThreadID == nil,
                     "Actual close advances Calendar to Shipments and disarms the previous reader")
        observe(nil, loaded: nil, category: nil)
        precondition(tour.practiceStep == .shipments, "Duplicate close notifications cannot skip a lesson")

        observe("practice-13", loaded: "practice-13", category: .shipments)
        precondition(tour.practiceRecordThreadID == "practice-13", "Shipment lesson accepts a different fixture too")
        observe("practice-1", loaded: "practice-1", category: nil)
        observe(nil, loaded: nil, category: nil)
        precondition(tour.practiceStep == .shipments, "Switching to unrelated mail disarms the old close")
        observe("practice-14", loaded: "practice-14", category: .shipments)
        observe(nil, loaded: nil, category: nil)
        precondition(tour.practiceStep == .otherCategories, "Closing any loaded shipment advances to the combined categories")

        tour.back()
        precondition(tour.practiceStep == .shipments && tour.practiceRecordThreadID == nil)
        observe(nil, loaded: nil, category: nil)
        precondition(tour.practiceStep == .shipments, "Back does not consume a previous completed reader")
        observe("practice-5", loaded: "practice-5", category: .shipments)
        tour.skipPracticeStep()
        observe(nil, loaded: nil, category: nil)
        precondition(tour.practiceStep == .otherCategories, "Skip disarms the reader before its close event")
        tour.cancel()
        observe("practice-5", loaded: "practice-5", category: .shipments)
        precondition(tour.practiceRecordThreadID == nil, "Canceled tours ignore late reader events")
        RehearsalMode.setEnabled(false)
    }

    @MainActor
    static func standaloneWithoutAccount() async throws {
        let store = AppStore.shared
        store.connectionAfterExit = .disconnected
        let tour = TourController()
        tour.replay(store: store, source: .rehearsal)
        try await settle()
        precondition(tour.active && RehearsalMode.isEnabled)
        tour.skip()
        try await settle()
        precondition(!RehearsalMode.isEnabled && !tour.active && tour.veil == nil,
                     "With no live board to summarize, leaving practice ends at the Connect gate")
        precondition(tour.hasLeftPractice, "The Connect gate skips the intro after practice")
        let entersBeforeConnect = store.enters
        tour.maybeStart()
        precondition(!tour.active, "A disconnected account cannot show a live summary")
        store.connStatus = .connected
        tour.maybeStart()
        precondition(tour.active && tour.phase == .summary && !RehearsalMode.isEnabled,
                     "Connecting after practice continues to the live summary")
        precondition(store.enters == entersBeforeConnect, "Connecting never repeats practice")
        tour.finish()
        tour.maybeStart()
        precondition(!tour.active, "The resumed summary finishes onboarding")
        store.connectionAfterExit = .connected
    }
}
