// Explicit user corrections. Kinds describe mail; destinations and external
// access are independent. Restricting agents never hides mail from its owner.
import Foundation

enum TriageAxis: String, Sendable, Hashable {
    case kinds, destinations
    case showInFye = "show_in_fye"
    case externalAccess = "external_access"

    var chipLabel: String {
        switch self {
        case .kinds: "kind"
        case .destinations: "place"
        case .showInFye: "attention"
        case .externalAccess: "agents"
        }
    }
}

/// List edits carry intent, so a pending or concurrently updated decision keeps
/// unrelated kinds and destinations. Scalar corrections remain explicit values.
struct TriageCorrectionRequest: Encodable {
    var field: String
    var value: Bool?
    var add: [String]?
    var remove: [String]?

    init(_ target: TriageTarget) {
        field = target.axis.rawValue
        switch target.axis {
        case .kinds, .destinations:
            add = target.removes ? [] : [target.value]
            remove = target.removes ? [target.value] : []
        case .showInFye, .externalAccess:
            value = target.value == "true"
        }
    }
}

struct TriageTarget: Identifiable, Hashable, Sendable {
    var axis: TriageAxis
    var value: String
    var label: String
    var hint: String
    var aliases: [String]
    var nudges: [String] = []
    var removes = false

    var id: String { "\(axis.rawValue):\(value):\(removes)" }

    /// List corrections preserve other kinds and overlapping destinations.
    func correctedValues(_ current: [String]) -> [String] {
        if removes { return current.filter { $0 != value } }
        return current.contains(value) ? current : current + [value]
    }
}

enum TriageTargets {
    private static let kindDefinitions: [(String, String, [String])] = [
        ("correspondence", "Correspondence", ["personal", "conversation", "reply"]),
        ("editorial", "Editorial", ["newsletter", "digest", "essay"]),
        ("promotional", "Promotion", ["sale", "offer", "marketing", "ad"]),
        ("bill", "Bill", ["invoice", "payment", "autopay"]),
        ("receipt", "Receipt", ["purchase", "confirmation"]),
        ("financial_update", "Financial update", ["statement", "banking", "transaction"]),
        ("delivery", "Delivery", ["shipment", "shipping", "tracking"]),
        ("event_reservation", "Event or reservation", ["calendar", "travel", "appointment"]),
        ("account_service", "Account or service", ["support", "service"]),
        ("authentication_security", "Authentication or security", ["auth", "login", "verification"]),
        ("general", "General", ["other"]),
    ]

    static let all: [TriageTarget] = [
        TriageTarget(axis: .showInFye, value: "true", label: "For your eyes",
            hint: "Show this thread in your attention list", aliases: ["fye", "eyes", "important", "attention"]),
        TriageTarget(axis: .showInFye, value: "false", label: "Remove from For your eyes",
            hint: "Keep the mail, remove it from your attention list", aliases: ["quiet", "noise", "ignore", "notimportant"]),
        TriageTarget(axis: .destinations, value: "reading", label: "Add to Reading",
            hint: "Keep extracted calendar, shipping, billing, and receipt facts", aliases: ["reading", "readlater"]),
        TriageTarget(axis: .destinations, value: "reading", label: "Remove from Reading",
            hint: "Keep other placements", aliases: ["notreading", "removereading"], removes: true),
        TriageTarget(axis: .externalAccess, value: "true", label: "Restrict agent access",
            hint: "Contains a code, reset link, or sign-in credential; you can still read it",
            aliases: ["sealed", "seal", "2fa", "otp", "code", "reset", "magiclink", "private"]),
        TriageTarget(axis: .externalAccess, value: "false", label: "Allow agent access",
            hint: "Contains no access-granting credential",
            aliases: ["unseal", "allowed", "allowagent", "notsealed"]),
    ] + kindDefinitions.flatMap { value, label, aliases in
        [
            TriageTarget(axis: .kinds, value: value, label: label,
                hint: "Add this kind without changing where the mail appears", aliases: aliases),
            TriageTarget(axis: .kinds, value: value, label: "Remove \(label.lowercased()) kind",
                hint: "Keep the other kinds and placements", aliases: ["not" + value], removes: true),
        ]
    }

    /// Normalize for matching: lowercase, and underscores/spaces are the same.
    private static func norm(_ s: String) -> String {
        s.lowercased().filter { $0 != " " && $0 != "_" && $0 != "-" }
    }

    /// Rank a target against what the user typed. Higher = better; 0 hides it.
    /// Deliberately boring — exact beats prefix beats substring, and value beats
    /// alias beats nudge — so which label Enter writes stays predictable.
    static func score(_ target: TriageTarget, query: String) -> Int {
        let q = norm(query)
        if q.isEmpty { return 1 }  // empty query: everything, in declaration order

        let value = norm(target.value)
        let label = norm(target.label)
        if value == q || label == q { return 100 }
        if value.hasPrefix(q) || label.hasPrefix(q) { return 80 }

        var best = 0
        for alias in target.aliases {
            let a = norm(alias)
            if a == q {
                best = max(best, 60)
            } else if a.hasPrefix(q) {
                best = max(best, 40)
            }
        }
        if best > 0 { return best }

        // A nudge ranks under every alias, so it can only ADD an option below
        // the one the typed word names — never replace it.
        for nudge in target.nudges {
            let n = norm(nudge)
            if n == q || n.hasPrefix(q) { return 30 }
        }

        if value.contains(q) || label.contains(q) { return 20 }
        return 0
    }

    /// One target's ranking row: declaration order + its score for a query.
    private struct Ranked {
        var index: Int
        var target: TriageTarget
        var score: Int
    }

    /// The ranked, filtered target list for a query. Stable within equal scores.
    static func match(_ query: String) -> [TriageTarget] {
        var ranked: [Ranked] = []
        for (index, target) in all.enumerated() {
            let s = score(target, query: query)
            if s > 0 { ranked.append(Ranked(index: index, target: target, score: s)) }
        }
        ranked.sort { a, b in a.score != b.score ? a.score > b.score : a.index < b.index }
        return ranked.map(\.target)
    }

    /// Human-facing label for a raw wire value, for showing what it WAS.
    static func label(axis: TriageAxis, value: String?) -> String {
        guard let value, !value.isEmpty else { return "unset" }
        return all.first { $0.axis == axis && $0.value == value }?.label ?? value
    }
}
