// FYE membership and order belong to the server, including dateless mail and
// older obligations. The badge and both apps must consume the same answer.
import Foundation

@main
@MainActor
struct NeedTodayTests {
    static func main() throws {
        let json = """
        {"version":2,"ranked_at":"2026-09-15T12:00:00Z","items":[
          {"message_id":9,"thread_id":"personal","from_addr":"friend@example.com",
           "subject":"Dinner","received_at":"2026-09-15T11:00:00Z","score":55,
           "decision":{"kinds":["correspondence"],"destinations":[],"summary":"Dinner invitation","reason":"Personal invitation"},
           "attention":{"show_in_fye":true,"state":"informational","summary":"Dinner invitation",
             "factors":{"urgency":0,"action_need":0,"personal_relevance":1,"importance":0.1,"attention_at":null}}},
          {"message_id":2,"thread_id":"bill","from_addr":"billing@example.com",
           "subject":"Invoice","received_at":"2026-09-01T11:00:00Z","score":50,
           "decision":{"kinds":["bill"],"destinations":["records"],"summary":"Invoice","reason":"Payment outstanding"},
           "attention":{"show_in_fye":true,"state":"needs_user","summary":"Pay invoice",
             "factors":{"urgency":1,"action_need":1,"personal_relevance":0,"importance":1,"attention_at":{"value":"2026-09-30","timezone":null}}}}
        ]}
        """
        let feed = try JSONDecoder().decode(AgentFeed.self, from: Data(json.utf8))
        let rows = feed.items.map(\.row)
        precondition(rows.map(\.id) == [9, 2], "importance must not reorder server results")
        precondition(NeedToday.count(rows) == 2, "dateless and future-dated FYE both count")
        precondition(NeedToday.count([]) == 0, "empty FYE clears the badge")
        precondition(rows[1].deadline == "2026-09-30", "preserve date-only precision")
        precondition(feed.items[1].decision.destinations == ["records"], "records may overlap FYE")
        precondition(feed.items[1].readingRow.one_line == "Invoice", "Reading uses message summary")
        let pending = try JSONDecoder().decode(AgentTriageInspection.self,
            from: Data("{\"decision\":null}".utf8))
        precondition(pending.decision == nil, "Explicit null means pending")
        let incompatible = try? JSONDecoder().decode(AgentTriageInspection.self,
            from: Data("{}".utf8))
        precondition(incompatible == nil, "Missing decision is not an empty placement list")
        var local = Calendar(identifier: .gregorian)
        local.timeZone = TimeZone(identifier: "America/Los_Angeles")!
        let lateDueDay = Fmt.date("2026-09-30T23:59:00-07:00")!
        precondition(Fmt.deadlineChip("2026-09-30", now: lateDueDay, calendar: local)?.overdue == false)
        precondition(Fmt.deadlineChip("2026-09-30", now: Fmt.date("2026-10-01T00:01:00-07:00")!, calendar: local)?.overdue == true)
        precondition(Fmt.deadlineChip("2026-09-30T12:00:00-07:00", now: lateDueDay, calendar: local)?.overdue == true, "Timestamp deadlines keep their exact instant")
        precondition(Fmt.calendarDay("2026-02-30", calendar: local) == nil)
        var projected = feed
        projected.items[0].decision.records = try JSONDecoder().decode([AgentRecordProposal].self, from: Data("""
        [{"kind":"receipt","merchant":"Shop","amount":12.5,"currency":"USD"},
         {"kind":"receipt","merchant":"Other Shop","amount":20,"currency":"USD"},
         {"kind":"event","title":"Dinner","start":{"value":"2026-10-01"}},
         {"kind":"financial_update","institution":"Bank","description":"Statement ready"}]
        """.utf8))
        precondition(projected.receipts.count == 2 && Set(projected.receipts.map(\.id)).count == 2)
        precondition(projected.receipts[0].amount == 12.5 && projected.receipts[0].message_id == 9)
        precondition(projected.calendar[0].starts_at == "2026-10-01", "Record date precision survives presentation")
        precondition(projected.banking[0].kind == .update, "Do not invent financial subtypes")
        projected.items[0].decision.kinds = ["promotional"]
        precondition(projected.marketing.count == 1 && projected.marketing[0].code == nil)
        projected.items.removeAll()
        precondition(projected.receipts.isEmpty && projected.calendar.isEmpty && projected.banking.isEmpty && projected.marketing.isEmpty, "Canonical empty responses clear every projection")
        let capabilities = try JSONDecoder().decode(TriageCapabilities.self, from: Data("""
        {"triage_version":2,"server_ranked_fye":true,"reading":true,"pending_message_read":true}
        """.utf8))
        precondition(capabilities.isSupported)
        var incompatibleDaemon = capabilities
        incompatibleDaemon.pending_message_read = false
        precondition(!incompatibleDaemon.isSupported, "Exact-message taps require the declared capability")
        incompatibleDaemon = capabilities
        incompatibleDaemon.triage_version = 1
        precondition(!incompatibleDaemon.isSupported, "Legacy daemons require upgrade before connecting")
        var inventory = rows[1]
        inventory.deadline = nil
        inventory.deadline_date = "2026-09-30"
        let decodedInventory = try JSONDecoder().decode(AttentionUpdate.self, from: JSONEncoder().encode(inventory))
        precondition(decodedInventory.displayDeadline == "2026-09-30", "Inventory date-only field survives decoding")
        precondition(Fmt.deadlineChip(decodedInventory.displayDeadline, now: lateDueDay, calendar: local)?.overdue == false)
        precondition(ProcessQueue.pending(snapshot: rows, live: rows, handled: []).map(\.id) == [9, 2])
        var changed = rows[1]
        changed.one_line = "Updated obligation"
        let queue = ProcessQueue.pending(snapshot: rows, live: [changed, rows[0]], handled: [9])
        precondition(queue.count == 1 && queue[0].one_line == "Updated obligation")
        changed.status = .done
        precondition(ProcessQueue.pending(snapshot: rows, live: [changed], handled: []).isEmpty)
        let bill = try JSONDecoder().decode(AgentRecordProposal.self, from: Data("""
        {"kind":"bill","merchant":"Utility","amount":124.50,"currency":"USD","due":{"value":"2026-09-30"},"autopay":true}
        """.utf8))
        precondition(bill.amount == 124.50 && bill.currency == "USD" && bill.autopay == true)
        precondition(bill.due?.value == "2026-09-30")
        print("ok: 28 agent feed, process, record, and capability checks passed")
    }
}
