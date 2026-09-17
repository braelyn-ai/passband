// Shared badge/headline count. Membership comes from the server's FYE list;
// the client never filters it again using deadlines or importance.
import Foundation

enum NeedToday {
    static let bandLimit = 200

    static func count(_ items: [AttentionUpdate], now: Date = Date()) -> Int {
        items.count
    }
}
