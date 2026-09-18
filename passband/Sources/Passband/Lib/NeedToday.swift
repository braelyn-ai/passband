// Shared badge/headline count. Membership comes from the server's FYE list;
// the client never filters it again using deadlines or importance.
import Foundation

enum NeedToday {
    static let bandLimit = 200

    static func count(_ items: [AttentionUpdate], now: Date = Date()) -> Int {
        items.count
    }
}

/// Process snapshots server order, but consumes current row content and state.
enum ProcessQueue {
    static func pending(snapshot: [AttentionUpdate], live: [AttentionUpdate], handled: Set<Int>) -> [AttentionUpdate] {
        let current = Dictionary(live.map { ($0.id, $0) }, uniquingKeysWith: { _, newest in newest })
        return snapshot.compactMap { row in
            guard !handled.contains(row.id), let latest = current[row.id], latest.status != .done else { return nil }
            return latest
        }
    }
}
