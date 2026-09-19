import Foundation

@main
struct ReadingTests {
    static func main() throws {
        func row(_ id: Int, sender: String, reason: String, date: String) -> AttentionUpdate {
            AttentionUpdate(id: id, thread_id: "thread-\(id)", tier: .signal,
                importance: 100, sender: sender, one_line: "Message \(id)", reason: reason,
                status: .new, surfaced_at: date)
        }
        let oldHuman = row(1, sender: "friend@example.com", reason: "Personal note",
            date: "2020-01-01T00:00:00Z")
        let receiptCopy = row(2, sender: "shop@example.com", reason: "Order confirmation / receipt",
            date: "2026-09-01T00:00:00Z")
        let recentHuman = row(3, sender: "friend@example.com", reason: "Sale promotion",
            date: "2026-09-02T00:00:00Z")
        let cards = Reading.derive(updates: [oldHuman, receiptCopy, recentHuman], rules: [])
        precondition(cards.count == 2, "No sender, copy, age, or tier may remove agent-selected mail")
        precondition(cards[0].items.map(\.id) == [3, 1], "Grouping retains every message, newest first")
        precondition(cards[1].items.map(\.id) == [2], "Receipt wording is not a client exclusion")
        let pruned = Reading.prune(cards, resolved: [3])
        precondition(pruned[0].latestThreadId == "thread-1" && pruned[0].count == 1)
        precondition(Reading.derive(updates: [], rules: []).isEmpty, "Empty canonical membership clears cards")
        print("ok: 5 canonical Reading checks passed")
    }
}
