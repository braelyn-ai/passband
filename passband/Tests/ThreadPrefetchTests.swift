import Foundation

// Narrow stand-ins for rendering/network dependencies. Tests compile and exercise
// the real ThreadPrefetch and AsyncMemo; no API or web frame is involved.
struct ClientThreadView: Sendable {
    var cache_allowed: Bool? = true
    var messages: [ClientMessage] = []
    var marker: Int = 0
}
struct ClientMessage: Sendable {
    var html: String?
    var allowsTrackers = false
    var attachmentList: [Int] = []
}
@MainActor final class APIClient {
    static let shared = APIClient()
    func getThread(_ id: String) async throws -> ClientThreadView { fatalError("inject loader") }
}
enum EmailWebView {
    struct Prepared: Sendable {
        static func cacheKey(_ html: String, _ allow: Bool, _ parts: [Int]) -> Int { 0 }
        static func make(from: String, allowTrackers: Bool, attachments: [Int]) -> Self { Self() }
    }
}
enum ThreadStyle: CaseIterable {
    case classic, bubbles
    func frameKey(_ id: Int) -> String { "\(self):\(id)" }
}

@MainActor private final class Requests {
    var pending: [CheckedContinuation<ClientThreadView, Error>] = []
    func load(_ id: String) async throws -> ClientThreadView {
        try await withCheckedThrowingContinuation { pending.append($0) }
    }
}

@main struct ThreadPrefetchTests {
    @MainActor static func spin() async { for _ in 0..<50 { await Task.yield() } }
    @MainActor static func main() async throws {
        let requests = Requests()
        let cache = ThreadPrefetch(loader: requests.load)
        cache.prefetch("same")
        await spin()
        let hero = Task { try await cache.fetch("same", fresh: 600) }
        let peek = Task { try await cache.fetch("same") }
        await spin()
        precondition(requests.pending.count == 1, "preload, hero and peek must share one GET")
        requests.pending[0].resume(returning: ClientThreadView(marker: 1))
        let heroView = try await hero.value
        let peekView = try await peek.value
        precondition(heroView.marker == 1 && peekView.marker == 1)
        _ = try await cache.fetch("same")
        precondition(requests.pending.count == 1, "successful fetch must populate TTL cache")

        let old = Task { try await cache.fetch("switch") }
        await spin()
        cache.wipe()
        let new = Task { try await cache.fetch("switch") }
        await spin()
        precondition(requests.pending.count == 3)
        requests.pending[1].resume(returning: ClientThreadView(marker: 2))
        do { _ = try await old.value; fatalError("old-account result escaped") }
        catch is CancellationError {}
        let join = Task { try await cache.fetch("switch") }
        await spin()
        precondition(requests.pending.count == 3, "old completion must not remove new task")
        requests.pending[2].resume(returning: ClientThreadView(marker: 3))
        _ = try await new.value
        let joined = try await join.value
        precondition(joined.marker == 3 && cache.cached("switch")?.marker == 3)

        let failed = Task { try await cache.fetch("retry") }
        await spin()
        requests.pending[3].resume(throwing: URLError(.notConnectedToInternet))
        do { _ = try await failed.value; fatalError("expected failure") }
        catch is URLError {}
        let retry = Task { try await cache.fetch("retry") }
        await spin()
        precondition(requests.pending.count == 5, "failed fetch must allow another attempt")
        requests.pending[4].resume(returning: ClientThreadView(cache_allowed: false))
        _ = try await retry.value
        precondition(cache.cached("retry") == nil, "restricted response must not be cached")
        print("ThreadPrefetch tests passed")
    }
}
