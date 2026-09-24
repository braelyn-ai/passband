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

    /// SupportedTime may carry a local wall time and a separate IANA zone.
    /// Explicit offsets win; missing zones use the user's current calendar.
    static func startDate(_ value: String?, timezone: String?, calendar: Calendar = .current) -> Date? {
        guard let value else { return nil }
        let iso = ISO8601DateFormatter()
        iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = iso.date(from: value) { return date }
        iso.formatOptions = [.withInternetDateTime]
        if let date = iso.date(from: value) { return date }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = timezone.flatMap(TimeZone.init(identifier:)) ?? calendar.timeZone
        formatter.isLenient = false
        for format in ["yyyy-MM-dd", "yyyy-MM-dd'T'HH:mm:ss", "yyyy-MM-dd'T'HH:mm", "yyyy-MM-dd'T'HH:mm:ss.SSS"] {
            formatter.dateFormat = format
            if let date = formatter.date(from: value), formatter.string(from: date) == value {
                return date
            }
        }
        return nil
    }

    func admits(account: String, item: Int, startValue: String?, timezone: String?,
                now: Date, calendar: Calendar = .current) -> Bool {
        var eventCalendar = calendar
        if let timezone, let zone = TimeZone(identifier: timezone) { eventCalendar.timeZone = zone }
        return admits(account: account, item: item,
                      start: Self.startDate(startValue, timezone: timezone, calendar: eventCalendar),
                      allDay: startValue?.count == 10, now: now, calendar: eventCalendar)
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
