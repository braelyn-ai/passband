// Compile with the real APIClient. The session transport is intercepted here:
// no test touches the network, the keychain, or account storage.
import Foundation
import os

@main
struct CredentialProbeTests {
    static func main() async {
        let client = APIClient()
        RehearsalMode.setEnabled(true)
        await expectFailure(client, kind: .badRequest)
        expect(ProbeProtocol.requests.withLock { $0 } == 0,
               "practice credential checks must fail before network access")
        let fixtureRequests = await RehearsalAPI.shared.requests
        expect(fixtureRequests == 0, "credential checks must not accept fictional stats")

        RehearsalMode.setEnabled(false)
        await expectFailure(client, kind: .network)
        expect(ProbeProtocol.requests.withLock { $0 } == 4,
               "live credential checks must use the candidate transport")
        let remainingFixtureRequests = await RehearsalAPI.shared.requests
        expect(remainingFixtureRequests == 0, "live failures must not fall back to practice")
        print("Credential probe tests passed")
    }

    static func expectFailure(_ client: APIClient, kind: APIErrorKind) async {
        do {
            try await client.probe(baseURL: "https://credential-probe.invalid", token: "test-only")
            fatalError("credential probe unexpectedly succeeded")
        } catch let error as APIError {
            expect(error.kind == kind, "wrong failure: \(error.kind), expected \(kind)")
        } catch {
            fatalError("unexpected error type: \(error)")
        }
    }

    static func expect(_ condition: Bool, _ message: String) {
        guard condition else { fatalError(message) }
    }
}

private final class ProbeProtocol: URLProtocol, @unchecked Sendable {
    static let requests = OSAllocatedUnfairLock(initialState: 0)
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.requests.withLock { $0 += 1 }
        client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
    }
    override func stopLoading() {}
}

enum Sessions {
    static func ephemeral(
        timeout: TimeInterval, resource: TimeInterval,
        cachePolicy: URLRequest.CachePolicy, emptyHeaders: Bool
    ) -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ProbeProtocol.self]
        return URLSession(configuration: config)
    }
}

actor RehearsalAPI {
    static let shared = RehearsalAPI()
    private(set) var requests = 0
    func response(for request: URLRequest) throws -> (Data, HTTPURLResponse) {
        requests += 1
        throw APIError(.unknown, 0, "Credential probe reached the fixture transport")
    }
}

// These UI-owned enums are irrelevant to credential validation, but appear in
// other APIClient method signatures.
enum SearchSortChoice: String, Sendable { case recent, bestMatch = "best_match" }
