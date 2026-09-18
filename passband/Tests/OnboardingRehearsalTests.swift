import Foundation

@main
struct OnboardingRehearsalTests {
    static func main() {
        var mailbox = OnboardingRehearsal()
        precondition(mailbox.messages.count == 18)
        precondition(Set(mailbox.messages.map(\.id)).count == 18)
        for category in OnboardingRehearsal.Category.allCases {
            precondition(mailbox.messages.filter { $0.category == category }.count == 3)
        }
        precondition(mailbox.messages.allSatisfy { !$0.body.isEmpty && !$0.isRead && !$0.isDone })
        let reading = mailbox.messages.filter { $0.category == .reading }
        precondition(reading.map(\.sender) == ["Cats Weekly", "Haightssion", "The Federal Overstatement"])
        precondition(reading.allSatisfy { $0.body.count > 500 })
        precondition(mailbox.messages.first { $0.id == 1 }!.body.contains("No reply needed"))
        mailbox.open(1)
        mailbox.setDone(1, done: true)
        precondition(mailbox.messages.first { $0.id == 1 }?.isRead == true)
        precondition(mailbox.messages.first { $0.id == 1 }?.isDone == true)
        mailbox.setDone(1, done: false)
        precondition(mailbox.messages.first { $0.id == 1 }?.isDone == false)
        mailbox.reset()
        precondition(mailbox.messages == OnboardingRehearsal().messages)
        print("Onboarding rehearsal tests passed")
    }
}
