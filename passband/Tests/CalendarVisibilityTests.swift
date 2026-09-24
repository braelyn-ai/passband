import Foundation

@main
@MainActor
struct CalendarVisibilityTests {
    static func main() {
        let suite = "CalendarVisibilityTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let window = CalendarVisibility(defaults: defaults)
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let future = now.addingTimeInterval(7 * 86400)
        func admits(_ time: Date, account: String = "a", start: Date? = nil,
                    allDay: Bool = false) -> Bool {
            window.admits(account: account, item: 1, start: start, allDay: allDay, now: time)
        }
        precondition(admits(now, start: future), "Unseen future events remain available")
        precondition(!admits(now, start: now.addingTimeInterval(-1)), "Past events are hidden")
        precondition(!admits(now, start: now), "Timed events expire at their start")
        precondition(admits(now), "Unknown dates remain until their viewing timer expires")
        window.markSeen(account: "a", item: 1, now: now)
        precondition(admits(now.addingTimeInterval(86399), start: future))
        window.markSeen(account: "a", item: 1, now: now.addingTimeInterval(3600))
        precondition(!admits(now.addingTimeInterval(86400), start: future), "Viewing again cannot extend retention")
        precondition(admits(now.addingTimeInterval(86400), account: "b", start: future), "Accounts are isolated")
        precondition(window.admits(account: "a", item: 2, start: future, allDay: false,
                                   now: now.addingTimeInterval(86400)), "Unseen items retain their own timer")
        let reopened = CalendarVisibility(defaults: defaults)
        precondition(!reopened.admits(account: "a", item: 1, start: future, allDay: false,
                                     now: now.addingTimeInterval(86400)), "First viewing survives reopening")
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "America/Los_Angeles")!
        let day = calendar.startOfDay(for: now)
        let tomorrow = calendar.date(byAdding: .day, value: 1, to: day)!
        precondition(window.admits(account: "b", item: 1, start: day, allDay: true,
                                   now: tomorrow.addingTimeInterval(-1), calendar: calendar))
        precondition(!window.admits(account: "b", item: 1, start: day, allDay: true,
                                    now: tomorrow, calendar: calendar))
        let zone = "America/Los_Angeles"
        let instant = CalendarVisibility.startDate("2026-09-15T22:05:00Z", timezone: nil)!
        precondition(CalendarVisibility.startDate("2026-09-15T15:05:00", timezone: zone) == instant)
        precondition(CalendarVisibility.startDate("2026-09-15T15:05:00", timezone: nil, calendar: calendar) == instant)
        precondition(CalendarVisibility.startDate("2026-09-15T22:05:00Z", timezone: zone) == instant)
        precondition(CalendarVisibility.startDate("2026-09-15T15:05:00-07:00", timezone: "UTC") == instant)
        precondition(CalendarVisibility.startDate("2026-02-30", timezone: zone) == nil)
        precondition(CalendarVisibility.startDate("unknown", timezone: zone) == nil)
        precondition(!window.admits(account: "c", item: 1, startValue: "2026-09-15T15:05:00",
                                    timezone: zone, now: instant.addingTimeInterval(1)))
        precondition(window.admits(account: "c", item: 1, startValue: "2026-09-15T15:05:00",
                                   timezone: zone, now: instant.addingTimeInterval(-1)))
        let midnight = CalendarVisibility.startDate("2026-09-16T07:00:00Z", timezone: nil)!
        precondition(window.admits(account: "c", item: 1, startValue: "2026-09-15",
                                   timezone: zone, now: midnight.addingTimeInterval(-1)))
        precondition(!window.admits(account: "c", item: 1, startValue: "2026-09-15",
                                    timezone: zone, now: midnight))
        print("Calendar visibility: 21 checks passed")
    }
}
