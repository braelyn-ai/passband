// Exercise the production poller with an intentionally cancellation-oblivious
// transport: the old account may answer after practice warmup has begun.
import Foundation

@main
struct SitrepPollerTests {
    @MainActor
    static func main() async {
        let poller = SitrepPoller.shared
        let api = APIClient.shared
        let store = AppStore.shared
        let old = Task { await poller.pull() }
        await until { api.requests == 1 }

        poller.stop()
        store.epoch += 1
        let fresh = Task { await poller.pull() }
        await until { api.requests == 2 }
        expect(api.pending.count == 2, "practice starts before the old request finishes")

        api.finish(1)
        _ = await old.value
        expect(store.sitrep.stats == nil, "stale response must not populate practice")
        expect(Badge.refreshes == 0, "stale response must not update badge")
        expect(store.lastRefresh == nil, "stale response must not mark a refresh")

        let joined = Task { await poller.pull() }
        for _ in 0..<100 { await Task.yield() }
        expect(api.requests == 2, "old completion must not clear the new in-flight task")
        api.finish(2)
        _ = await (fresh.value, joined.value)
        expect(store.sitrep.stats?.total == 2, "fresh fixture response reaches the board")
        expect(Badge.refreshes == 1, "only the current request has side effects")

        // stop() itself protects the store, even without an epoch bump.
        let canceled = Task { await poller.pull() }
        await until { api.requests == 3 }
        poller.stop()
        api.finish(3)
        _ = await canceled.value
        expect(store.sitrep.stats?.total == 2, "canceled response is ignored without an epoch change")

        let failed = Task { await poller.pull() }
        await until { api.requests == 4 }
        poller.stop()
        api.fail(4)
        _ = await failed.value
        expect(store.refreshError == nil, "canceled errors cannot poison the new mailbox")

        store.connStatus = .loading
        _ = await poller.pull()
        expect(api.requests == 4, "transition loading gate prevents requests")
        print("SitrepPoller tests passed")
    }

    @MainActor
    static func until(_ condition: () -> Bool) async {
        for _ in 0..<10_000 {
            if condition() { return }
            await Task.yield()
        }
        fatalError("Timed out waiting for fake transport")
    }

    static func expect(_ condition: @autoclosure () -> Bool, _ message: String) {
        if !condition() { fatalError(message) }
    }
}

// Narrow collaborators; SitrepPoller.swift and Concurrency.swift are unmodified
// production sources in this executable. No network, credentials, or app host.
enum NeedToday { static let bandLimit = 100 }
enum Platform { static let didBecomeActiveNotification = Notification.Name("test.focus") }
enum ConnectionStatus { case connected, loading }
enum Band { case standing, new, open }
struct UpdatesParams: Sendable { let band: Band; let limit: Int }
struct AttentionUpdate: Equatable, Sendable {
    var id: Int; var thread_id: String; var senderString: String
}
struct Feed: Sendable { var items: [FeedItem] = []; var total_count: Int? = 0 }
struct FeedItem: Sendable { var row: AttentionUpdate }
struct Updates: Sendable { var items: [AttentionUpdate] = [] }
struct Stats: Equatable, Sendable {
    var total: Int
    /// Optional, as on the wire: a daemon too old to name its mailbox sends
    /// nothing, and the seed below must compile against that shape.
    var account_email: String? = "practice@example.test"
    var sealed = 0
    var bands = Bands()
    var tier_counts: [String: Int] = [:]
    struct Bands: Equatable, Sendable { var standing = 0; var new = 0; var open = 0 }
}
struct SitrepData: Equatable {
    var standing: [AttentionUpdate] = []
    var new: [AttentionUpdate] = []
    var open: [AttentionUpdate] = []
    var stats: Stats?
    var sealed: [Int] = []
    var totalCount: Int?
}
struct OpenThreadSummary { var threadId: String; var newestMessageId: Int }
enum ErrorKind { case unknown }
struct APIError: Error { var message: String; var kind: ErrorKind }
struct RefreshError { var message: String; var kind: ErrorKind }
@MainActor final class AppStore {
    static let shared = AppStore()
    var connStatus = ConnectionStatus.connected
    var epoch = 0
    var sitrep = SitrepData()
    var currentThreadSummary: OpenThreadSummary?
    var refreshError: RefreshError?
    var lastRefresh: Date?
    var selectedId: Int?
    var orderedIds: [Int] { [] }
    func isCurrent(_ e: Int) -> Bool { epoch == e }
    func refreshZones(force: Bool = false) async {}
    func noteThreadArrival(thread: String, message: Int, sender: String) {}
}
@MainActor final class APIClient {
    static let shared = APIClient()
    var requests = 0
    var pending: [Int: CheckedContinuation<Stats, Error>] = [:]
    func getStats() async throws -> Stats {
        requests += 1
        let id = requests
        return try await withCheckedThrowingContinuation { pending[id] = $0 }
    }
    func finish(_ id: Int) { pending.removeValue(forKey: id)!.resume(returning: Stats(total: id)) }
    func fail(_ id: Int) { pending.removeValue(forKey: id)!.resume(throwing: APIError(message: "old failure", kind: .unknown)) }
    func getFeed(destination: String, limit: Int) async throws -> Feed { Feed() }
    func getUpdates(_ params: UpdatesParams) async throws -> Updates { Updates() }
    func listSealed() async throws -> [Int] { [] }
    func refreshMail() async throws {}
}
@MainActor enum Badge {
    static var refreshes = 0
    static func set(_ count: Int) { refreshes += 1 }
    static func refresh(_ rows: [AttentionUpdate]) { refreshes += 1 }
}
@MainActor final class Prefs {
    static let shared = Prefs()
    func seedUserName(fromEmail: String?) {}
}
enum Analytics {
    static func capture(_ event: String) {}
    static func daily(_ event: String, _ properties: [String: Int]) {}
}
@MainActor final class ImageWarmer {
    static let shared = ImageWarmer()
    func noteSitrepLanded() {}
}
