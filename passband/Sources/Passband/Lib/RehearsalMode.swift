import Foundation
import os

/// The practice inbox is a MODE of the one app, not a second process. While it
/// is on, the API client answers from `RehearsalAPI`, the dashboard and reader
/// draw fixture mail, and a handful of surfaces (sidebar navigation, favicon
/// fetches, the sync chip) hold still. Preferences, credentials and telemetry
/// are the real ones throughout: the person practicing is the same person who
/// will use the inbox a minute later, so their theme, their name and their
/// funnel carry across instead of living in a throwaway domain.
///
/// Only `AppStore.enterPractice` / `exitPractice` flip the flag, and both fence
/// the store's epoch first, so a fixture answer can never land in the live read
/// model or the other way round.
enum RehearsalMode {
    /// Launched as a tester rehearsal (`--onboarding-rehearsal` or
    /// `PASSBAND_ONBOARDING_REHEARSAL=1`): the app opens on the intro and the
    /// practice inbox without loading an account, shows the rehearsal controls,
    /// and sends no telemetry. Customer onboarding never sets this.
    static let launchedStandalone =
        ProcessInfo.processInfo.arguments.contains("--onboarding-rehearsal")
        || ProcessInfo.processInfo.environment["PASSBAND_ONBOARDING_REHEARSAL"] == "1"

    /// Exercise the real connection flow before entering practice; never
    /// validate connection credentials using the fixture transport.
    static let includesConnection = launchedStandalone
        && ProcessInfo.processInfo.arguments.contains("--rehearse-connection")

    private static let state = OSAllocatedUnfairLock(initialState: launchedStandalone && !includesConnection)

    /// Whether the practice inbox is on screen right now. Read from every
    /// layer the fixture transport has to intercept — the API client's actor,
    /// view bodies, the poller — so it is a lock, not a main-actor property.
    static var isEnabled: Bool { state.withLock { $0 } }

    static func setEnabled(_ enabled: Bool) { state.withLock { $0 = enabled } }
}
