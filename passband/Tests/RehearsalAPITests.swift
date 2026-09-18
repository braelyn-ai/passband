import Foundation

@main
struct RehearsalAPITests {
    static func main() async throws {
        // test.sh runs from the passband directory; the photographs are read
        // from the checkout because a bare test binary has no bundle.
        let api = RehearsalAPI(
            referenceDate: Date(timeIntervalSince1970: 1_789_200_000),
            resourceDirectory: URL(fileURLWithPath: "Sources/Passband/Resources", isDirectory: true))
        func request(_ path: String, method: String = "GET", body: [String: String]? = nil) -> URLRequest {
            var request = URLRequest(url: URL(string: "https://rehearsal.invalid/client/" + path)!)
            request.httpMethod = method
            if let body { request.httpBody = try! JSONSerialization.data(withJSONObject: body) }
            return request
        }
        func read<T: Decodable>(_ path: String, as: T.Type) async throws -> T {
            let (data, response) = try await api.response(for: request(path))
            precondition(response.statusCode == 200)
            return try JSONDecoder().decode(T.self, from: data)
        }
        let fye = try await read("v2/feed?destination=fye", as: AgentFeed.self)
        precondition(Set(fye.items.map(\.message_id)) == [1, 4, 11])
        let reading = try await read("v2/feed?destination=reading", as: AgentFeed.self)
        precondition(reading.items.count == 3)
        let records = try await read("v2/feed?destination=records", as: AgentFeed.self)
        precondition(records.items.count == 12)
        let exact = try await read("v2/messages/1", as: HumanMessageEnvelope.self)
        precondition(exact.message_id == 1 && exact.thread.messages[0].id == 1)
        let initial = try await read("stats", as: StoreStats.self)
        precondition(initial.total == 18 && initial.bands.standing == 3 && initial.catch_up == nil)
        let standing = try await read("updates?band=standing", as: Page<AttentionUpdate>.self)
        precondition(Set(standing.items.map(\.id)) == [1, 4, 11])
        let maya = standing.items.first { $0.id == 1 }!
        precondition(maya.tier == .signal && maya.deadline == nil)
        let mayaThread = try await read("thread/practice-1", as: ClientThreadView.self)
        precondition(mayaThread.messages[0].content.contains("No reply needed"))
        let exfed = try await read("thread/practice-5", as: ClientThreadView.self)
        precondition(exfed.messages[0].from_addr == "tracking@exfed.example")
        precondition(exfed.messages[0].content.contains("Shipper: Rainforest"))
        for id in [13, 14] {
            let merchant = try await read("thread/practice-\(id)", as: ClientThreadView.self)
            precondition(merchant.messages[0].from_name == "Rainforest" && merchant.messages[0].from_addr == "orders@rainforest.example")
        }
        let receipts = try await read("receipts", as: [Receipt].self)
        let shipments = try await read("shipments", as: [Shipment].self)
        let banking = try await read("banking", as: [BankingRecord].self)
        let calendar = try await read("calendar", as: [CalendarUpdate].self)
        precondition(receipts.count == 3 && shipments.count == 3 && banking.count == 3 && calendar.count == 3)
        let zip = calendar.first { $0.id == 17 }!
        precondition(zip.kind == .invite && zip.event_title?.contains("Zip") == true && zip.organizer == "Maya Chen")
        let zipThread = try await read("thread/practice-17", as: ClientThreadView.self)
        precondition(zipThread.messages[0].content.contains("Join Zip Meeting"))
        precondition(zipThread.messages[0].content.contains("Meeting ID: 824 5107 3926"))
        precondition(zipThread.messages[0].html?.contains("Zip") == true)
        let noise = try await read("updates?tier=noise", as: Page<AttentionUpdate>.self)
        precondition(noise.items.filter { $0.reason.hasPrefix("Newsletter:") && $0.status != .done }.count == 3)
        let snapshot = await api.snapshot()
        for message in snapshot.messages {
            let thread = try await read("thread/practice-\(message.id)", as: ClientThreadView.self)
            precondition(thread.messages.count == 1 && thread.messages[0].id == message.id)
            precondition(!thread.messages[0].content.contains("{{") && thread.subject == message.subject)
            if !message.body.contains("{{") { precondition(thread.messages[0].content == message.body) }
            if message.category == .newsletters {
                let html = thread.messages[0].html ?? ""
                precondition(html.contains("<table") && html.contains("<h1"))
                precondition(!html.contains("<script") && !html.contains("src=\"http"))
                let prefix = "data:image/jpeg;base64,"
                guard let start = html.range(of: prefix), let end = html[start.upperBound...].firstIndex(of: "\""),
                      let photo = Data(base64Encoded: String(html[start.upperBound..<end])) else {
                    preconditionFailure("Every newsletter must contain its bundled JPEG photo")
                }
                precondition(photo.starts(with: [0xff, 0xd8]) && photo.count > 10_000)
                let repeated = try await read("thread/practice-\(message.id)", as: ClientThreadView.self)
                precondition(repeated.messages[0].html == html)
            }
            if message.lane == .records {
                let html = thread.messages[0].html ?? ""
                precondition(html.contains("<table") && !html.contains("{{"))
                precondition(!html.contains("<script") && !html.contains("<img") && !html.contains("href="))
            }
        }
        _ = try await api.response(for: request("updates/1/status", method: "POST", body: ["status": "done"]))
        let done = try await read("updates?band=standing", as: Page<AttentionUpdate>.self)
        precondition(!done.items.contains { $0.id == 1 })
        _ = try await api.response(for: request("updates/1/status", method: "POST", body: ["status": "open"]))
        let restored = try await read("updates?band=standing", as: Page<AttentionUpdate>.self)
        precondition(restored.items.contains { $0.id == 1 })
        let (ruleData, _) = try await api.response(for: request("rules", method: "POST", body: ["match_pattern": "*@brightly.example", "want": "Mute terms-of-service updates", "disposition": "squelch"]))
        let created = try JSONDecoder().decode(CreatedRule.self, from: ruleData)
        let rules = try await read("rules", as: [SenderRule].self)
        precondition(rules.count == 1 && rules[0].id == created.rule_id)
        let muted = try await read("updates?band=standing", as: Page<AttentionUpdate>.self)
        precondition(!muted.items.contains { $0.id == 11 })
        let actuallyMuted = await api.suppresses(messageID: 11, viaRuleID: created.rule_id)
        precondition(actuallyMuted)
        let (unrelatedData, _) = try await api.response(for: request("rules", method: "POST", body: ["match_pattern": "*@different.example", "want": "Mute this sender", "disposition": "squelch"]))
        let unrelated = try JSONDecoder().decode(CreatedRule.self, from: unrelatedData)
        let unrelatedMuted = await api.suppresses(messageID: 11, viaRuleID: unrelated.rule_id)
        precondition(!unrelatedMuted, "A different saved pattern must not claim an existing rule's effect")
        let (allowData, _) = try await api.response(for: request("rules", method: "POST", body: ["match_pattern": "updates@brightly.example", "want": "Keep these visible", "disposition": "surface"]))
        let allow = try JSONDecoder().decode(CreatedRule.self, from: allowData)
        let allowMuted = await api.suppresses(messageID: 11, viaRuleID: allow.rule_id)
        let overriddenMute = await api.suppresses(messageID: 11, viaRuleID: created.rule_id)
        precondition(!allowMuted && !overriddenMute, "An explicit allow and rule specificity must be respected")
        _ = try await api.response(for: request("rules/\(allow.rule_id)", method: "PUT", body: ["match_pattern": "updates@brightly.example", "want": "Only important terms changes", "disposition": "filtered"]))
        let filteredMuted = await api.suppresses(messageID: 11, viaRuleID: allow.rule_id)
        precondition(!filteredMuted, "Stored free-text filters must not pretend local fixtures evaluated AI instructions")
        _ = try await api.response(for: request("rules/\(allow.rule_id)", method: "DELETE"))
        _ = try await api.response(for: request("rules/\(created.rule_id)", method: "DELETE"))
        let unmuted = try await read("updates?band=standing", as: Page<AttentionUpdate>.self)
        precondition(unmuted.items.contains { $0.id == 11 })
        _ = try await api.response(for: request("shipments/5/clear", method: "POST"))
        let cleared = try await read("shipments", as: [Shipment].self)
        precondition(cleared.count == 2)
        do {
            _ = try await api.response(for: request("actions/send", method: "POST", body: ["body": "Do not send"]))
            preconditionFailure("Unsupported writes must fail locally")
        } catch let error as APIError { precondition(error.status == 400) }
        _ = try await api.response(for: request("rules", method: "POST", body: ["match_pattern": "*@brightly.example", "want": "Mute updates", "disposition": "squelch"]))
        _ = try await api.response(for: request("updates/1/status", method: "POST", body: ["status": "done"]))
        await api.reset()
        let reset = try await read("stats", as: StoreStats.self)
        let resetRules = try await read("rules", as: [SenderRule].self)
        let resetShipments = try await read("shipments", as: [Shipment].self)
        let resetSnapshot = await api.snapshot()
        precondition(reset.total == 18 && reset.bands.standing == 3 && resetRules.isEmpty && resetShipments.count == 3)
        precondition(resetSnapshot.messages.allSatisfy { !$0.isDone && !$0.isRead })
        print("Rehearsal API tests passed")
    }
}
