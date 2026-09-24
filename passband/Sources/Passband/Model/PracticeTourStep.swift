import Foundation

/// Guided product lessons teach real interactions with the practice mailbox.
enum PracticeTourStep: Int, CaseIterable, Sendable {
    case welcome, needsYou, openMaya, done, undo
    case calendar, shipments, otherCategories, reading
    case learning, openBrightly, rule, ruleSaved, wrap

    var targets: [TourTarget] {
        switch self {
        case .needsYou, .learning: [.eyes]
        case .openMaya: [.maya]
        case .openBrightly: [.brightly]
        case .calendar: [.calendar]
        case .shipments: [.shipments]
        case .otherCategories: [.banking, .receipts]
        case .reading: [.reading]
        default: []
        }
    }

    var exampleID: Int? {
        switch self {
        case .openMaya, .done: 1
        case .calendar: 17
        case .shipments: 5
        case .reading: 3
        case .openBrightly, .rule: 11
        default: nil
        }
    }

    var isInteraction: Bool {
        switch self {
        case .openMaya, .done, .undo, .calendar, .openBrightly, .rule: true
        default: false
        }
    }

    var recordCategory: TourTarget? {
        switch self {
        case .calendar: .calendar
        case .shipments: .shipments
        default: nil
        }
    }

    var title: String {
        switch self {
        case .welcome: "Your inbox, with room to breathe."
        case .needsYou: "Start with what needs you."
        case .openMaya: "One message. One small step."
        case .done: "Finished with it?"
        case .undo: "You can bring it right back."
        case .calendar: "Your plans, in one place."
        case .shipments: "Keep an eye on what’s coming."
        case .otherCategories: "The other categories follow suit."
        case .reading: "Something to read, when you like."
        case .learning: "We learn what matters to you."
        case .openBrightly: "A notice you don’t need."
        case .rule: "Your words become a rule."
        case .ruleSaved: "Your rule is saved."
        case .wrap: "A little less to carry."
        }
    }

    /// A key in braces, `{e}`, renders as an inline keycap in the guide card.
    var explanation: String {
        switch self {
        case .welcome:
            "Let’s try a few things in this practice inbox. The mail is fictional, but the mailbox is the real Passband experience."
        case .needsYou:
            "Think of For Your Eyes as your to-do list. Reading a message keeps it here. Marking it done clears it from your board."
        case .openMaya:
            "Maya has a short update for you. Click her message in For Your Eyes to read it."
        case .done:
            "Maya’s note needs a read, not a reply. Press {e} when you’re done. It leaves your board and stays in your mail."
        case .undo:
            "That message is off your board. Now that the reader is closed, you can undo to bring it right back."
        case .calendar:
            "Invitations, reservations, and changes to your plans belong here. Click any calendar item to read its original email."
        case .shipments:
            "See what’s on its way without treating every delivery update as a task. Open one if you like, or choose Next."
        case .otherCategories:
            "Banking keeps account updates together. Receipts keep purchases close at hand. The details are here when you need them."
        case .reading:
            "Your reading has its own space. Read something interesting whenever you have a moment."
        case .learning:
            "Your actions help Passband learn what matters. You can also nudge the AI directly with a smart rule, written in your own words."
        case .openBrightly:
            "The sender is required to notify you of legal updates. You can’t unsubscribe, but most people never read these updates. Click Brightly’s highlighted message."
        case .rule:
            "Press {t} to write a smart rule. Try “I don’t need terms-of-service updates.” Keep Mute selected and save."
        case .ruleSaved:
            "Your preference is saved for this practice sender. You can change smart rules whenever you need to."
        case .wrap:
            "Your inbox is ready to explore. Start with what needs you, revisit the details, or try another preference at your own pace."
        }
    }
}
