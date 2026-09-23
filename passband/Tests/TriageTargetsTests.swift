import Foundation

@main
struct TriageTargetsTests {
    static func main() throws {
        let reading = TriageTargets.all.first { $0.axis == .destinations && $0.value == "reading" && !$0.removes }!
        precondition(reading.correctedValues([]) == ["reading"])
        precondition(reading.correctedValues(["reading"]) == ["reading"])
        let remove = TriageTargets.all.first { $0.axis == .destinations && $0.value == "reading" && $0.removes }!
        precondition(remove.correctedValues(["reading"]) == [])
        precondition(!TriageTargets.all.contains { $0.axis == .destinations && $0.value == "records" })
        let bill = TriageTargets.match("bill").first!
        precondition(bill.axis == .kinds && !bill.removes)
        precondition(bill.correctedValues(["receipt"]) == ["receipt", "bill"])
        let auth = TriageTargets.match("auth").first!
        precondition(auth.axis == .kinds, "Auth kind never implicitly restricts access")
        let code = TriageTargets.match("2fa").first!
        precondition(code.axis == .externalAccess && code.value == "true")
        let attention = TriageTargets.match("fye").first!
        precondition(attention.axis == .showInFye && attention.value == "true")
        precondition(Set(TriageTargets.all.map(\.id)).count == TriageTargets.all.count)
        let addPayload = try JSONSerialization.jsonObject(with: JSONEncoder().encode(TriageCorrectionRequest(reading))) as! [String: Any]
        precondition(addPayload["add"] as? [String] == ["reading"])
        precondition(addPayload["remove"] as? [String] == [])
        precondition(addPayload["value"] == nil, "Pending edits never pin an absolute destination list")
        let removePayload = try JSONSerialization.jsonObject(with: JSONEncoder().encode(TriageCorrectionRequest(remove))) as! [String: Any]
        precondition(removePayload["remove"] as? [String] == ["reading"])
        precondition(removePayload["value"] == nil, "Removing Reading never removes concurrent Records")
        let scalar = try JSONSerialization.jsonObject(with: JSONEncoder().encode(TriageCorrectionRequest(attention))) as! [String: Any]
        precondition(scalar["value"] as? Bool == true && scalar["add"] == nil)
        print("ok: 15 independent triage correction checks passed")
    }
}
