import Foundation

/// First viewing is durable and account-scoped. Repeated views never extend it.
@MainActor
final class CalendarVisibility {
    static let shared = CalendarVisibility()
    static let lifetime: TimeInterval = 24 * 3600
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) { self.defaults = defaults }

    private func key(account: String, item: Int) -> String {
        "passband.calendar.firstSeen.\(account).\(item)"
    }

    func markSeen(account: String, item: Int, now: Date = Date()) {
        let key = key(account: account, item: item)
        guard defaults.object(forKey: key) == nil else { return }
        defaults.set(now.timeIntervalSince1970, forKey: key)
    }

    func admits(account: String, item: Int, start: Date?, allDay: Bool,
                now: Date, calendar: Calendar = .current) -> Bool {
        if let start {
            // A date-only event remains current through its entire local day.
            let cutoff = allDay
                ? calendar.date(byAdding: .day, value: 1, to: calendar.startOfDay(for: start)) ?? start
                : start
            guard now < cutoff else { return false }
        }
        let key = key(account: account, item: item)
        guard defaults.object(forKey: key) != nil else { return true }
        let seen = Date(timeIntervalSince1970: defaults.double(forKey: key))
        return now < seen.addingTimeInterval(Self.lifetime)
    }
}
