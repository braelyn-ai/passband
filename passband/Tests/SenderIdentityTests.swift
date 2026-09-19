// Which senders leave the device, and which never do.
//
// `eligibleFaviconDomain` is the whole privacy boundary in SenderIdentity: it
// answers nil for a human correspondent, and that nil is the reason the
// correspondent graph stays local. A heuristic guarding something that quiet
// is asserted rather than reasoned about — a widening that reaches one address
// too far does not crash, does not fail a build, and shows up as an icon
// nobody looks at twice.
//
// The display-name arm is the new one, and it is the one most of these are
// about: a brand that signs its bulk mail with its own name is a brand,
// however its ESP spelled the mailbox.

import Foundation

@main
@MainActor
struct SenderIdentityTests {
    static var failures = 0
    static var checks = 0

    static func main() {
        brandsAreRecognised()
        peopleAreNot()
        theBoundaryHolds()
        theKnownRemainder()
        namesStayCorrect()
        notificationsNeverShowAnAddress()

        if failures > 0 {
            print("FAILED: \(failures) of \(checks) checks")
            exit(1)
        }
        print("ok: \(checks) checks passed")
    }

    static func brandsAreRecognised() {
        // The original arm: the local-part IS the domain's label.
        expect(SenderID.isBrand("eBay <eBay@eBay.com>"), "local-part equal to the label")
        expect(SenderID.isBrand("ebay@ebay.com"), "the same with no display name at all")

        // The arm this suite exists for. Bulk mail leaves from whatever
        // mailbox the sender's ESP minted, and none of these is on
        // `robotLocals` — before the display name counted, every one of them
        // was treated as a person. (`express@airbnb.com` is the witness that
        // matters: nothing else in this file would have caught it.)
        expect(SenderID.isBrand("Airbnb <express@airbnb.com>"), "display name over a routing box")
        expect(SenderID.isBrand("STRIPE <e@stripe.com>"), "case is not part of the match")
        expect(SenderID.isBrand("Booking.com <news-2938@booking.com>"), "a name carrying its TLD")

        // Word for word, which is what makes a HYPHENATED brand domain work:
        // the tokens line up on both sides.
        expect(SenderID.isBrand("Blue Apron <hi@blue-apron.com>"), "two words, one hyphen")
        expect(SenderID.isBrand("T-Mobile <news@t-mobile.com>"), "hyphens on both sides")
        expect(
            SenderID.isBrand("Marks and Spencer <news@marks-and-spencer.co.uk>"),
            "three words under a compound suffix")
        expect(SenderID.isBrand("Nestlé <news@nestle.com>"), "diacritics fold before matching")

        // A robot local-part still stands on its own, with or without a name.
        expect(SenderID.isRobot("no-reply@stripe.com"), "the robot arm is untouched")
        expect(SenderID.isRobot("\"Chase\" <no.reply.alerts@chase.com>"), "squashed markers too")
    }

    static func peopleAreNot() {
        expect(!SenderID.isBrand("Sarah Chen <sarah@acme.com>"), "a person at a company")
        expect(!SenderID.isBrand("sarah@acme.com"), "a bare human address")

        // THE ONES THAT MATTER. A person on their own name-domain is the
        // common shape of a personal address — freelancers, consultants,
        // family domains — and the first cut of the display-name arm squashed
        // the space out of "Jane Doe" and read it as the brand janedoe.com.
        // The tokens are what tell them apart: a brand buys the hyphen, a
        // person buys the concatenation.
        expect(!SenderID.isBrand("Jane Doe <jane@janedoe.com>"), "a person on their own domain")
        expect(!SenderID.isBrand("Bob Smith <bob@bobsmith.com>"), "and again, concatenated")
        expect(
            !SenderID.isBrand("Sarah Chen <sarah@sarahchen.co.uk>"),
            "and under a compound suffix")

        // A consumer host's domain belongs to the mailbox provider, so a
        // display name matching it asserts nothing about the sender.
        expect(!SenderID.isBrand("Gmail <friend@gmail.com>"), "a consumer host is never the brand")
        expect(!SenderID.isBrand("Proton <friend@proton.me>"), "nor a privacy host")
        expect(!SenderID.isBrand("Sarah <sarah@gmail.com>"), "a first name on a consumer host")

        // EXACT, token for token. A prefix match would read the address below
        // as the brand Sam, which is a person's mail leaving the device.
        expect(!SenderID.isBrand("Samuel Smith <ssmith@sam.com>"), "no prefix matching")
        // And a qualified team name is not the company asserting itself.
        expect(!SenderID.isBrand("Airbnb Support <express@airbnb.com>"), "\"Airbnb Support\" is not")
        expect(!SenderID.isBrand("Airbnb, Inc. <express@airbnb.com>"), "nor \"Airbnb, Inc.\"")

        // A display name that is just the address again is not a name.
        expect(!SenderID.isBrand("sarah@acme.com <sarah@acme.com>"), "the address is not a name")
    }

    /// The remainder, pinned as BEHAVIOUR rather than left to be discovered.
    ///
    /// A one-word display name equal to your own registrable domain is
    /// structurally identical to "Airbnb", and nothing available at this layer
    /// separates them. These assert what the code does today so that the day
    /// `sender_known` reaches `Avatar`, the person who closes it sees exactly
    /// which cases they closed — and so that nobody reads the arm above as
    /// airtight.
    static func theKnownRemainder() {
        expect(
            SenderID.isBrand("Anderson <john@anderson.com>"),
            "KNOWN: a one-word surname on its own domain still reads as a brand")
        expect(
            SenderID.eligibleFaviconDomain("Chen <sc@chen.com>") == "chen.com",
            "KNOWN: and its domain is therefore resolved over the network")
    }

    static func theBoundaryHolds() {
        // What actually reaches the network, which is the only question that
        // matters here. A domain, never an address, and only for a sender that
        // named itself a service.
        expect(
            SenderID.eligibleFaviconDomain("Airbnb <express@airbnb.com>") == "airbnb.com",
            "a brand resolves to its registrable domain")
        expect(
            SenderID.eligibleFaviconDomain("no-reply@stripe.com") == "stripe.com",
            "so does a robot")
        expect(
            SenderID.eligibleFaviconDomain("Sarah Chen <sarah@acme.com>") == nil,
            "A HUMAN CORRESPONDENT NEVER LEAVES THE DEVICE")
        expect(
            SenderID.eligibleFaviconDomain("Samuel Smith <ssmith@sam.com>") == nil,
            "nor one who nearly matched their own domain")
        expect(
            SenderID.eligibleFaviconDomain("Jane Doe <jane@janedoe.com>") == nil,
            "NOR ONE WHOSE NAME IS THEIR DOMAIN")
        expect(
            SenderID.eligibleFaviconDomain("express@airbnb.com") == nil,
            "an unnamed routing box stays local — the brand is not asserted")
        expect(
            SenderID.eligibleFaviconDomain("Bob") == nil,
            "and a sender with no domain has nothing to ask about")
    }

    static func namesStayCorrect() {
        // `displayName` calls `isBrand` on its way to an answer, so a change to
        // one is a change to the other. These pin that the rows still read the
        // way they did.
        expect(
            SenderID.displayName("Airbnb <express@airbnb.com>") == "Airbnb",
            "a display name still wins outright")
        expect(
            SenderID.displayName("ebay@ebay.com") == "ebay",
            "a brand with no name shows its local-part as given")
        expect(
            SenderID.displayName("no-reply@stripe.com") == "Stripe",
            "a robot shows the capitalized domain label")
        expect(
            SenderID.displayName("sarah@acme.com") == "sarah@acme.com",
            "and everyone else shows the address")

        // Initials come off the NAME, never the full address.
        expect(SenderID.initials("Sarah Chen <sarah@acme.com>") == "SC", "two words, two letters")
        expect(SenderID.initials("bboynton97@gmail.com") == "BB", "never the domain's letters")
    }

    /// `readableName` is what a banner's title is built from, and the one
    /// promise it makes is the one `displayName` deliberately does not: no
    /// address, ever. Each case below is a shape that has actually landed in a
    /// notification title.
    static func notificationsNeverShowAnAddress() {
        let r = SenderID.readableName
        expect(r("Sarah Chen <sarah@acme.com>") == "Sarah Chen", "a display name wins outright")
        expect(r("Airbnb <express@airbnb.com>") == "Airbnb", "so does a brand's own name")
        expect(r("ebay@ebay.com") == "ebay", "a brand with no name shows its local-part")
        expect(r("no-reply@stripe.com") == "Stripe", "a robot mailbox shows its domain label")
        expect(r("alerts@mail.chase.com") == "Chase", "through a mail subdomain")
        expect(
            r("bounce-1234-5678@em.brand.com") == "Brand",
            "an ESP routing box is its domain, not its token")
        expect(r("sarah.chen@acme.com") == "Sarah Chen", "a dotted local-part is a person's name")
        expect(r("sarah_chen@gmail.com") == "Sarah Chen", "at a consumer host too")
        expect(r("sarah@acme.com") == "Acme", "a lone word at a real domain shows the domain")
        expect(r("jsmith@acme.com") == "Acme", "because Jsmith in bold is not a name")
        expect(r("bboynton97@gmail.com") == "bboynton97", "an opaque gmail local stands alone")
        expect(r("x7k2q9@outlook.com") == "x7k2q9", "the provider's name adds nothing")

        // Display names that are not names.
        expect(
            r("No Reply <no-reply@accounts.google.com>") == "Google",
            "a robot word as the display name is no display name")
        expect(
            r("Notifications <notifications@github.com>") == "Github",
            "nor is a bare mailbox role")
        expect(
            r("GitHub Notifications <notifications@github.com>") == "GitHub Notifications",
            "but a brand plus its role is fine")
        expect(
            r("notifications@github.com <noreply@github.com>") == "Github",
            "an address used as a display name is not a name")
        expect(
            r("Sarah Chen (sarah@acme.com) <sarah@acme.com>") == "Sarah Chen",
            "a name with its address in parentheses keeps the name")
        expect(
            r("=?UTF-8?Q?Caf=C3=A9?= <hi@cafe.com>") == "Cafe",
            "an undecoded encoded-word is bytes, not a name")
        expect(r("acme.com <no-reply@acme.com>") == "Acme", "the domain as a name is the brand")
        expect(
            r("John =?UTF-8?Q?M=C3=BCller?= <john@acme.com>") == "John",
            "an encoded-word anywhere in the name is dropped, the rest kept")
        expect(
            r("'sarah@acme.com' via Team <team@googlegroups.com>") == "Acme via Team",
            "an address inside a name is labelled, not deleted")
        expect(r("Sarah @ Acme <sarah@acme.com>") == "Sarah @ Acme", "a lone @ is punctuation")
        expect(
            r("Sarah Chen [sarah@acme.com] <sarah@acme.com>") == "Sarah Chen",
            "a trailing bracketed address is dropped")

        // Role mailboxes are functions, not people: never a fake employee.
        expect(r("account-security@apple.com") == "Apple", "not Account Security")
        expect(r("order-confirmation@amazon.com") == "Amazon", "not Order Confirmation")
        expect(r("customer.service@chase.com") == "Chase", "not Customer Service")
        expect(r("ship-confirm@amazon.com") == "Amazon", "not Ship Confirm")
        expect(r("hr-team@acme.com") == "Acme", "not Hr Team")
        expect(r("hr-team@gmail.com") == "hr-team", "and at a consumer host, the local as given")

        // Hyphenated domains are names with hyphens in them, not typos.
        expect(r("info@marks-and-spencer.co.uk") == "Marks-And-Spencer", "per hyphen token")
        expect(r("jane@t-mobile.com") == "T-Mobile", "and the row's robot arm reads the same way:")
        expect(SenderID.displayName("no-reply@t-mobile.com") == "T-Mobile", "T-Mobile, not T-mobile")
        expect(r("Bob") == "Bob", "and a sender with no address at all is what it says")

        for sender in [
            "sarah@acme.com", "bounce-1234-5678@em.brand.com", "x7k2q9@outlook.com",
            "notifications@github.com <noreply@github.com>", "\"\" <a@b.co>",
            "'sarah@acme.com' via Team <team@googlegroups.com>",
            "John =?UTF-8?Q?M=C3=BCller?= <john@acme.com>", "account-security@apple.com",
            "=?UTF-8?Q?x?= <a@b.co>", "sarah@acme.com <sarah@acme.com>",
        ] {
            expect(!r(sender).contains("@"), "never an address: \(sender)")
            expect(!r(sender).isEmpty, "never blank: \(sender)")
        }

        // Rows are unchanged: the address is still the row's fallback.
        expect(
            SenderID.displayName("bounce-1234-5678@em.brand.com") == "bounce-1234-5678@em.brand.com",
            "readableName is the banner's rule, not the row's")
    }

    // MARK: - harness

    static func expect(_ ok: Bool, _ label: String) {
        checks += 1
        if !ok {
            failures += 1
            print("FAIL: \(label)")
        }
    }
}
