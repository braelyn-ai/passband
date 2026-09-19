import Foundation

@main
struct CapabilityProbeTests {
    static func main() {
        let offline = APIError(.network, 0, "offline")
        let unavailable = APIError(.server, 503, "unavailable")
        var retry = CapabilityProbeRetry()
        precondition(retry.delay(after: offline) == 1)
        precondition(retry.delay(after: unavailable) == 3)
        precondition(retry.delay(after: offline) == 10)
        precondition(retry.delay(after: unavailable) == nil, "A switch has bounded retry work")
        for permanent in [
            APIError(.unknown, 0, "incompatible triage version"),
            APIError(.notFound, 404, "missing capabilities"),
            APIError(.unauthorized, 401, "rejected token"),
            APIError(.forbidden, 403, "denied"),
        ] {
            var permanentRetry = CapabilityProbeRetry()
            precondition(permanentRetry.delay(after: permanent) == nil)
        }
        var cancelled = CapabilityProbeRetry()
        precondition(cancelled.delay(after: CancellationError()) == nil, "Cancellation never restarts a request")
        var futureAttempt = CapabilityProbeRetry()
        precondition(futureAttempt.delay(after: offline) == 1, "A later explicit connection attempt has a new retry budget")
        precondition(CapabilityProbeRetry.isTransient(unavailable), "Exhausting short retries does not misclassify a boot outage as incompatibility")
        precondition(!CapabilityProbeRetry.isTransient(APIError(.unknown, 0, "incompatible")))
        print("ok: 12 capability retry checks passed")
    }
}
