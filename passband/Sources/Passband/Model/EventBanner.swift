// What a notification for one event SAYS, and the userInfo keys a tap on it
// routes by. Split out of Notifier.swift because there are now two posters of
// the same banner and they must agree exactly: the app, from the live event
// feed, and the notification service extension, rewriting a push that arrived
// while the app was not running. A banner whose copy or whose routing keys
// depended on which of the two posted it would be a visible seam.
//
// Pure by construction: same event in, same banner out. Nothing here posts,
// reads the keychain, or knows an account exists — `Notifier` and the extension
// each fold the account in on their own side.
//
// It also answers the question that comes BEFORE the copy, and for the same
// reason: `routing(for:)`. A sealed event is not an event banner at all (see
// docs/NOTIFY.md §11.6), and the two posters plus the live feed have to agree
// about that or one of them puts a dead tap on a lock screen.

import Foundation

enum EventBanner {
    // MARK: - userInfo keys

    /// A tap routes on the thread id, WITHIN the account named by the account
    /// id. Both are read on whatever queue the system delivers the tap to, and
    /// both must survive being written to disk by the system and read back into
    /// a later launch — hence strings, and hence the account id as its uuid
    /// string rather than as a UUID.
    static let threadKey = "passband.thread_id"
    static let eventKey = "passband.event_id"
    static let messageKey = "passband.message_id"
    static let authKey = "passband.is_auth"
    /// The posting account's uuid, as a string (userInfo has to survive being
    /// written to disk by the system and read back into a later launch).
    static let accountKey = "passband.account_id"
    /// Where the tap goes when there is no thread to open. Carried ONLY by the
    /// background auth banners; its absence is how every event banner says
    /// "open the thread named in `threadKey`".
    static let routeKey = "passband.route"
    static let authRoute = "auth"
    /// The Settings test banner. It routes nowhere, but it is marked because
    /// the system has to be told to DRAW it — see `presentation` — and because
    /// a tap on it is not a human opening their mail and must not be counted
    /// as one.
    static let testRoute = "test"

    /// The relay's original push survives when device enrichment times out.
    /// Keep its account-qualified event handle so a tap can resolve the email.
    struct UnresolvedPush: Equatable, Sendable {
        var accountId: UUID
        var eventId: Int
    }

    static func unresolvedPush(_ raw: String?) -> UnresolvedPush? {
        guard let raw, let colon = raw.lastIndex(of: ":"),
            let account = UUID(uuidString: String(raw[..<colon])),
            let event = Int(raw[raw.index(after: colon)...]), event > 0 else { return nil }
        return UnresolvedPush(accountId: account, eventId: event)
    }

    // MARK: - routing

    /// What one frame off the event feed IS. The whole decision, in one pure
    /// function, because it is the fork the notify lane added and the two
    /// answers could hardly be further apart: one opens a thread, the other
    /// must never touch one.
    enum Routing: Equatable, Sendable {
        /// The ordinary event: a banner naming the sender, whose tap opens the
        /// thread the mail is in.
        case threadBanner
        /// Auth mail. The event is a TRIGGER and not a notification: the row
        /// carries no subject and its `created_at` is when triage emitted it,
        /// not when the mail arrived, so nothing about the banner may be built
        /// from it beyond the kind and the sender. `/client/sealed` is the
        /// source of truth for this legacy lookup route.
        case authSignal(SealedKind)
    }

    /// Route one event. `sealed_kind` present is the entire test — see
    /// docs/NOTIFY.md §11.6. Deliberately NOT a check on tier, kind or
    /// importance: the daemon stamps a sealed row `urgent`/`signal` like any
    /// other urgent mail, so those fields cannot tell the two apart, and a
    /// sealed event routed as a thread banner would put a dead tap on screen
    /// (`thread_guard_and_subject` 404s a sealed thread) beside the poller's
    /// own banner for the same code.
    static func routing(for event: Event) -> Routing {
        guard let kind = event.sealed_kind else { return .threadBanner }
        return .authSignal(kind)
    }

    static func shouldPresentForeground(appActive: Bool, windowVisible: Bool, isTest: Bool, isAuth: Bool) -> Bool {
        isTest || isAuth || !(appActive && windowVisible)
    }

    // MARK: - content mapping

    /// The display copy for one event. Pure: same event in, same banner out.
    struct Copy: Equatable {
        var title: String
        var subtitle: String
        var body: String
        var threadIdentifier: String
        var sound: Bool
    }

    static func copy(for event: Event) -> Copy {
        // "Sarah Chen <sarah@acme.com>" is not a notification title, and
        // neither is "bounce-1234@em.brand.com": `readableName` never answers
        // with an address, which is the one promise a bold line on a lock
        // screen has to keep. The exact address is one tap away in the thread.
        let sender = flatten(SenderID.readableName(event.sender), max: 64)
        let summary = flatten(event.one_line, max: 240)
        // Coalescing key: events on one thread stack into ONE group. The
        // fallback must be unique, or an empty thread id would glue unrelated
        // mail together.
        let group =
            event.thread_id.isEmpty ? "passband.event.\(event.id)" : event.thread_id

        // A read receipt on the user's OWN tracked mail, and NOT a triage
        // verdict: `sender` is the account's own address (using it as the title
        // would read as mail from yourself), `one_line` is the sent subject, and
        // `importance` is a placeholder. It renders on its own terms.
        if event.kind == .opened {
            return Copy(
                title: "Opened",
                subtitle: "",
                body: summary.isEmpty ? "Someone opened your email." : "Opened: \(summary)",
                threadIdentifier: group,
                // News, not an obligation — no chime.
                sound: false)
        }

        let subtitle: String
        switch event.kind {
        // The time-bound kinds get a second line saying WHY the banner is worth
        // the interruption, but NOT the date. The event's deadline is a
        // snapshot taken at triage, and a banner sits on a lock screen for
        // hours: "due today" read tomorrow morning is wrong, and "2d PAST DUE"
        // on mail the human already handled is a nag. The thread has the live
        // chip; the banner says only that there is one.
        case .urgent: subtitle = "needs attention"
        case .deadline: subtitle = "has a deadline"
        // No second line for the ordinary case: a subtitle on every
        // notification is a subtitle that means nothing. `.opened` returned
        // above and is listed only to keep this switch exhaustive.
        case .surfaced, .opened: subtitle = ""
        }

        return Copy(
            title: sender.isEmpty ? "Passband" : sender,
            subtitle: subtitle,
            // An empty one_line means triage stored no summary; say something
            // true rather than posting a blank banner.
            body: summary.isEmpty ? "New mail worth your attention." : summary,
            threadIdentifier: group,
            // Sound only for the time-bound kinds — a chime per surfaced email
            // is how a notification stream gets muted wholesale.
            sound: event.isAuth || event.kind != .surfaced)
    }

    /// The AUTH banner's copy: a mailbox has just been sent a login code (or a
    /// reset, or a sign-in alert), and the only thing this notification exists
    /// to say is which mailbox to go to and who wants the code.
    ///
    /// WHAT IS DELIBERATELY NOT IN IT: the code, and the subject line that so
    /// often IS the code ("725104 is your Acme code" is a real subject). The
    /// only two inputs are the kind and the from address — both metadata, both
    /// the same class `/client/sealed` already serves — so there is no way for
    /// a body to reach a lock screen through here even by accident. A reveal is
    /// audited server-side and belongs to the account the human is looking at.
    ///
    /// THREE POSTERS, ONE COPY: the background watcher's poll (`postAuth`), the
    /// live feed's sealed event, and the notification service extension
    /// rewriting a push that arrived while the app was not running. Nothing in
    /// the wording may depend on which of them got there first.
    ///
    /// `accountName` is nil where the poster does not have one — the extension
    /// reads credentials out of the shared keychain and has no access to the
    /// app's account labels — and then the label stands alone rather than
    /// trailing an empty separator.
    static func authCopy(kind: SealedKind?, sender: String, accountName: String?) -> Copy {
        // The kind leads, the mailbox follows: WHICH account this is happening
        // in is the entire reason the banner is worth reading, and `AuthCopy`
        // is the app's one vocabulary for the other half ("sealed" is internal
        // jargon and never reaches a human).
        let label = AuthCopy.label(kind)
        let who = flatten(SenderID.readableName(sender), max: 64)
        return Copy(
            title: accountName.map { "\(label) · \($0)" } ?? label,
            subtitle: "",
            body: who.isEmpty ? "New auth mail." : "from \(who)",
            // One group per account's auth mail: a login code and the sign-in
            // alert behind it are the same conversation. Account-prefixed by
            // the poster, exactly as an event's group is, because two daemons'
            // auth mail is not one conversation.
            threadIdentifier: authGroup,
            // Always a chime. A login code is the definition of time-bound — it
            // expires while you are not looking at it.
            sound: true)
    }

    /// The coalescing group every auth banner shares, before the poster
    /// prefixes it with the account.
    static let authGroup = "passband.auth"

    /// Collapse to one line and cap the length: notification text is a small
    /// fixed box.
    static func flatten(_ s: String, max: Int) -> String {
        let flat = s.split(whereSeparator: { $0.isNewline || $0 == "\t" })
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return Fmt.truncate(flat, max)
    }
}

/// Keeps the most recent explicit tap until credentials and the account are ready.
/// A tap arriving during bootstrap must not be rejected against an unloaded index.
struct NotificationTapQueue<Target> {
    struct Tap {
        var target: Target
        var accountId: UUID?
    }
    private(set) var pending: Tap?
    private var parked = false

    mutating func enqueue(_ target: Target, accountId: UUID?) {
        pending = Tap(target: target, accountId: accountId)
        parked = false
    }

    /// Failed switches wait for a successful connection or another explicit tap.
    /// A newer tap that arrived during the switch is already a fresh request.
    mutating func park(_ tap: Tap) {
        guard pending == nil else { return }
        pending = tap
        parked = true
    }

    mutating func connectionBecameReady() { parked = false }

    mutating func take(connected: Bool) -> Tap? {
        guard connected, !parked else { return nil }
        defer { pending = nil }
        return pending
    }
}
