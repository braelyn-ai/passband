// The connect screen. Two ways in, one destination: a bearer token in the OS
// keychain, proved against /client/stats before it is stored.
//
// TWO PLACES SHOW IT, and `purpose` is the whole difference. As the GATE it is
// the window: this install has no identity, so it also teaches what a daemon is
// and, on success, becomes the identity. As the ADD ACCOUNT sheet it sits over
// a working app and adds a SECOND daemon beside the one already live — same
// form, same proof, but it must not touch the connection state the shell behind
// it is standing on (see AppStore.addAccount).
//
// PAIRING is the default. `squelchd pair` prints a code, this screen trades it
// for a token minted for THIS Mac alone — one that shows up by name in
// `squelchd token list` and dies on `squelchd token revoke`. Pasting a raw
// token stays available as the advanced path, because a self-hosted daemon's
// SQUELCH_API_TOKEN is still a first-class way in and always will be.
//
// Neither the code nor the token is ever logged, echoed into an error, or held
// anywhere but this view's own state on its way to the keychain.

import SwiftUI

/// The hosted control plane, and the two doors it opens.
///
/// SIGN IN is for somebody whose mailbox already exists: Google names them, the
/// control plane finds the daemon that mailbox owns, and the browser comes back
/// with a `passband://pair` link. No invite code anywhere in it, because an
/// invite provisions a tenant and this person's tenant is already running.
///
/// SIGN UP is the invite-code form, and it is the only place a code is asked
/// for. Both actions share the connection chooser and open their browser route.
private enum Hosted {
    // Send the rendered appearance, including a resolved system preference.
    // The account site preserves this choice through OAuth and pairing (#214).
    static func signIn(theme: ColorScheme) -> String {
        "https://signup.passband.app/app/auth?theme=\(theme == .dark ? "dark" : "light")"
    }
    static func signUp(theme: ColorScheme) -> String {
        "https://signup.passband.app?theme=\(theme == .dark ? "dark" : "light")"
    }
}

/// Which way in the screen is showing.
private enum ConnectMode: Hashable { case pair, token }

/// The first decision a fresh install makes. It changes the explanation and
/// defaults around the credential form, never the credential protocol itself:
/// hosted and self-hosted daemons expose the same human door.
private enum HostingChoice: Hashable { case hosted, selfHosted }

/// Progressive disclosure for the first account. Add Account deliberately
/// skips this — someone adding a second daemon already knows what Passband is.
private enum ConnectGateStep: Hashable {
    case introduction
    case welcome
    case selfHostGuide
    /// nil means a complete pair link brought us here. Its URL is authoritative,
    /// but it does not reliably reveal whether the daemon is hosted.
    case credentials(HostingChoice?)
}

/// Why this screen is up. See the file header — it decides what a success
/// MEANS, and nothing else about the form.
enum ConnectPurpose: Hashable {
    /// The app has no identity; connecting becomes it.
    case gate
    /// The app is connected; connecting adds another account beside it.
    case addAccount
}

/// Field focus, so Enter walks the form instead of dead-ending. `submit` is the
/// button itself: a deep link fills the form and lands here, because pairing is
/// never something a link does on its own.
private enum ConnectField: Hashable { case url, code, device, token, name, submit }

struct ConnectView: View {
    @Environment(AppStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme

    /// Defaults to the gate, so the two shells that mount it as the whole
    /// window say nothing about it.
    var purpose: ConnectPurpose = .gate

    @State private var mode: ConnectMode = .pair
    @State private var gateStep: ConnectGateStep = .introduction
    @State private var url = "http://127.0.0.1:8848"
    @State private var code = ""
    @State private var deviceName = Pairing.defaultDeviceName()
    @State private var token = ""
    /// The human's name for this account. Optional everywhere: an empty label
    /// leaves the switcher showing the daemon's host:port, which is a real
    /// answer rather than a placeholder.
    @State private var accountLabel = ""
    /// An add-account attempt is in flight. The gate reads `store.connStatus`
    /// for this, but adding deliberately never moves it — the shell behind the
    /// sheet is standing on it.
    @State private var adding = false
    @State private var retryingSavedConnection = false
    /// The failure from an add. Kept out of `store.connError`, which belongs to
    /// the gate and would still be sitting there next time one opened.
    @State private var addError: String?
    /// A claim is in flight. Separate from `connStatus`, which only starts
    /// moving once pairing has produced a token to test.
    @State private var claiming = false
    /// The pairing failure, which is this view's own — the store never sees the
    /// code or its rejection.
    @State private var pairError: String?
    /// A token a claim already minted that `connect` has not accepted yet. Held
    /// because a retry must re-run the PROBE, not the claim: claiming again
    /// spends another of the code's attempts and mints a second token nobody
    /// holds, which only the operator can clear with `squelchd token revoke`.
    /// Dropped once connect succeeds, and whenever the user edits the url or
    /// the code, since a token one daemon minted is nothing to another.
    @State private var heldToken: String?
    /// A deep link filled the form and stopped. Rings the button that is
    /// waiting for the press the link will never make for the user.
    @State private var linkArmed = false
    @State private var arrivedViaPairLink = false
    @State private var showingPairingDetails = false
    /// The gate was mounted because a saved connection could not be restored
    /// after the practice inbox. Fixed at mount: the retry it offers is about
    /// the keychain, not about whatever error the form shows later, so it
    /// must outlive the form's own habit of clearing `store.connError`.
    @State private var savedConnectionFailed = false
    /// The analytics id a hosted deep link carried, held until the pairing it
    /// belongs to actually LANDS. It is a claim about who this person is, and
    /// the only proof of that claim is a successful connect to the daemon the
    /// same link named — so it waits here rather than being adopted on arrival.
    /// Cleared alongside `heldToken`/`linkArmed` for the same reason they are:
    /// once a human has hand-edited the url or the code, the form is no longer
    /// that link's claim about anything.
    @State private var linkAid: String?
    @State private var pairingHelp = false
    @FocusState private var focus: ConnectField?

    private var busy: Bool { claiming || adding || retryingSavedConnection || store.connStatus == .connecting }

    /// One error line, whichever half produced it. A pairing failure wins: it is
    /// the more recent thing the user did.
    private var errorText: String? { pairError ?? addError ?? store.connError }

    private var canSubmit: Bool {
        guard !busy, !url.trimmed.isEmpty else { return false }
        switch mode {
        // A held token is a claim already paid for, and the code that bought it
        // is spent and cleared, so the code field is not what gates the retry.
        case .pair:
            return heldToken != nil
                || (Pairing.looksComplete(code) && !deviceName.trimmed.isEmpty)
        case .token: return !token.trimmed.isEmpty
        }
    }

    var body: some View {
        Group {
            switch purpose {
            case .gate: gateBody
            // A sheet is sized by its content, so the card's own measure is the
            // sheet's — no filling the screen, and no getting-started pane: an
            // install adding a SECOND daemon plainly knows what the first one
            // was.
            case .addAccount: connectCard.padding(24)
            }
        }
        // A link can arrive before this view mounts (the app was launched by
        // one, or the sheet is being opened BY one) or while it is up, so both
        // entry points are covered.
        .task {
            // Commit the initial route before the form can edit/clear an
            // error. Later error changes must never send it back to welcome.
            if purpose == .gate {
                if case .introduction = gateStep, store.tour.hasLeftPractice, store.connError != nil {
                    // The saved account's own host goes in the form, so a
                    // hosted person is not handed a localhost self-host form
                    // with a pairing-link instruction for a link that never came.
                    savedConnectionFailed = true
                    if let host = Self.savedHost {
                        url = (Self.isHostedHost(host) ? "https://" : "http://") + host
                    } else {
                        url = ""
                    }
                }
                gateStep = effectiveGateStep
            }
            applyPairLink(store.pairLink)
        }
        .onChange(of: store.pairLink) { _, link in applyPairLink(link) }
        // A link that arrived while the form was busy stayed parked on the
        // store; it is applied the moment the form can take it.
        .onChange(of: busy) { _, nowBusy in
            if !nowBusy { applyPairLink(store.pairLink) }
        }
    }

    /// A practice exit may need credentials, but never the intro again.
    private var effectiveGateStep: ConnectGateStep {
        if case .introduction = gateStep, store.tour.hasLeftPractice {
            // A saved connection that failed to restore needs the form and
            // its existing error, not the first-time hosting introduction —
            // and the form for the KIND of daemon that account was.
            guard store.connError != nil else { return .welcome }
            guard let host = Self.savedHost else { return .credentials(nil) }
            return .credentials(Self.isHostedHost(host) ? .hosted : .selfHosted)
        }
        return gateStep
    }

    /// The live account's host as the index remembers it, for a gate that
    /// exists because that account's credentials could not be read.
    private static var savedHost: String? {
        let host = AccountManager.shared.active?.displayHost ?? ""
        return host.isEmpty ? nil : host
    }

    private static func isHostedHost(_ host: String) -> Bool {
        host.hasSuffix("passband.email")
    }

    /// The welcome screen's retry, only when there is a saved connection to
    /// retry. Built here rather than inline: an optional closure conjured by
    /// a ternary inside the view builder is more than the type checker will
    /// resolve in one expression.
    private var savedRetry: (() -> Void)? {
        guard savedConnectionFailed else { return nil }
        return { retrySavedConnection() }
    }

    private var gateBody: some View {
        ZStack {
            switch effectiveGateStep {
            case .introduction:
                OnboardingIntroView { gateStep = .welcome }
            case .welcome:
                WelcomeGate(
                    retrySaved: savedRetry,
                    retrying: retryingSavedConnection,
                    login: {
                        url = ""
                        mode = .pair
                        arrivedViaPairLink = false
                        gateStep = .credentials(.hosted)
                        Opener.open(Hosted.signIn(theme: colorScheme))
                    },
                    selfHostedLogin: {
                        url = "http://127.0.0.1:8848"
                        mode = .pair
                        gateStep = .credentials(.selfHosted)
                    })
            case .selfHostGuide:
                SelfHostGuide(
                    back: { gateStep = .welcome },
                    continueToForm: {
                        mode = .pair
                        gateStep = .credentials(.selfHosted)
                    })
            case .credentials:
                connectCard
            }
        }
        // The phone's screen margin. On the Mac the card is a fixed measure
        // floating in a large window and needs none.
        #if !os(macOS)
            .padding(.horizontal, 16)
        #endif
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .overlay(alignment: .bottomTrailing) {
            SetupThemeToggle()
                .padding(20)
        }
    }

    private var connectCard: some View {
            VStack(spacing: 0) {
                header
                credentialContent

                if let errorText {
                    Label(errorText, systemImage: "exclamationmark.triangle.fill")
                        .font(.system(size: 12))
                        .foregroundStyle(Palette.danger)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 12)
                }

                if purpose == .gate, savedConnectionFailed {
                    Button(retryingSavedConnection ? "Trying saved connection…" : "Try saved connection again") {
                        retrySavedConnection()
                    }
                    .disabled(busy || store.accountActionsBlocked)
                    .padding(.top, 12)
                }

                bottomActions.padding(.top, 20)
            }
            .padding(30)
            // A window can always be 440pt wide; a phone cannot. Same intended
            // measure either way — the Mac states it, the phone treats it as a
            // ceiling and takes whatever the screen leaves (see body's margin).
            #if os(macOS)
                .frame(width: 440)
            #else
                .frame(maxWidth: 440)
            #endif
            .passbandGlass(
                purpose == .gate ? .chrome : .pane,
                cornerRadius: 24,
                tint: purpose == .gate ? Palette.glassTint.opacity(0.35) : Palette.glassTintStrong)
            .shadow(color: .black.opacity(0.3), radius: 50, y: 24)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(headerTitle)
                .font(Typo.serif(purpose == .gate ? 34 : 30, weight: .medium))
                .foregroundStyle(Palette.ink)
            Text(
                purpose == .gate
                    ? headerSubtitle
                    : "one daemon per mailbox. this one joins the accounts you already have."
            )
            .font(.system(size: 13))
            .foregroundStyle(Palette.inkFaint)
            .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.bottom, 22)
    }

    private var hostingChoice: HostingChoice? {
        guard purpose == .gate, case .credentials(let choice) = gateStep else { return nil }
        return choice
    }

    /// Re-read the index and the keychain the way boot does. The gate is up
    /// because that read failed once (a denied access panel, most often), and
    /// a second answer is a click away without re-pairing anything.
    private func retrySavedConnection() {
        guard !busy, !store.accountActionsBlocked else { return }
        retryingSavedConnection = true
        Task { @MainActor in
            await store.loadSettings()
            retryingSavedConnection = false
        }
    }

    private var headerTitle: String {
        guard purpose == .gate else { return "add account" }
        if arrivedViaPairLink { return "Make yourself at home." }
        switch hostingChoice {
        case .hosted: return "Connect your account."
        case .selfHosted: return "connect self-hosted"
        case nil: return "pair this device"
        }
    }

    private var headerSubtitle: String {
        if purpose == .gate, arrivedViaPairLink {
            return "Give this device and account names that feel familiar."
        }
        switch hostingChoice {
        case .hosted:
            return "Finish logging in in your browser. If you aren’t brought back automatically, enter your server URL and pairing code below."
        case .selfHosted:
            return "Run squelchd pair on your server, then enter what it prints."
        case nil:
            return "Check the server below, then confirm the pairing link."
        }
    }

    @ViewBuilder private var credentialContent: some View {
        if arrivedViaPairLink && mode == .pair {
            linkedPairForm
        } else {
            if purpose == .addAccount || hostingChoice == .selfHosted {
                GlassSegmented(
                    options: [(ConnectMode.pair, "pair with code"), (.token, "api token")],
                    selection: modeBinding)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.bottom, 16)
            }
            switch mode {
            case .pair: pairForm
            case .token: tokenForm
            }
            nameField.padding(.top, 14)
            if hostingChoice == .hosted {
                Button("Open login in browser again") { Opener.open(Hosted.signIn(theme: colorScheme)) }
                    .buttonStyle(.plain)
                    .font(.system(size: 12))
                    .foregroundStyle(Palette.inkDim)
                    .padding(.top, 14)
            }
            if purpose == .gate, hostingChoice == .selfHosted {
                Button("Need help setting up a server?") { gateStep = .selfHostGuide }
                    .buttonStyle(.plain)
                    .font(.system(size: 11))
                    .foregroundStyle(Palette.inkFaint)
                    .padding(.top, 14)
            }
        }
    }

    private var bottomActions: some View {
        HStack(spacing: 12) {
            Button(purpose == .addAccount ? "cancel" : "back") { goBack() }
                .disabled(busy)
            Spacer()
            Button(buttonLabel) { submit() }
                .buttonStyle(.borderedProminent)
                .tint(Palette.accent)
                .disabled(!canSubmit)
                .focused($focus, equals: .submit)
                .overlay {
                    if linkArmed {
                        RoundedRectangle(cornerRadius: 7, style: .continuous)
                            .strokeBorder(Palette.accent, lineWidth: 2)
                            .padding(-4)
                            .allowsHitTesting(false)
                    }
                }
        }
    }

    private func goBack() {
        guard purpose == .gate else { dismiss(); return }
        // The whole of a link's claim goes with the screen it filled: a code
        // and a held token belong to the daemon the link named, and the
        // analytics id it carried is a claim about a pairing that did not
        // happen. Leaving any of them behind would let the self-hosted form
        // present a hosted link's code, ringed and ready, against localhost.
        arrivedViaPairLink = false
        showingPairingDetails = false
        code = ""
        heldToken = nil
        linkArmed = false
        linkAid = nil
        pairError = nil
        addError = nil
        store.connError = nil
        gateStep = .welcome
    }

    /// The account's name, optional in both purposes. It matters most in the
    /// sheet — a second row in the switcher wants a word a human picked — but
    /// the first account is allowed one too, since it is about to have company.
    private var nameField: some View {
        Field(label: "account name") {
            TextField("optional, defaults to the server host", text: $accountLabel)
                .textFieldStyle(.plain)
                .autocorrectionDisabled()
                .focused($focus, equals: .name)
                .onSubmit { submit() }
        }
    }

    // MARK: - forms

    private var linkedPairForm: some View {
        VStack(alignment: .leading, spacing: 18) {
            Field(label: "Device name") {
                TextField("This Mac", text: $deviceName)
                    .textFieldStyle(.plain)
                    .font(.system(size: 16, weight: .medium))
                    .autocorrectionDisabled()
                    .focused($focus, equals: .device)
                    .onSubmit { focus = .name }
            }
            Field(label: "Account name") {
                TextField("Personal, work, or something else", text: $accountLabel)
                    .textFieldStyle(.plain)
                    .font(.system(size: 16, weight: .medium))
                    .autocorrectionDisabled()
                    .focused($focus, equals: .name)
                    .onSubmit { submit() }
            }
            Text("Account name is optional. You can rename it later.")
                .font(.system(size: 11))
                .foregroundStyle(Palette.inkFaint)
                .padding(.top, -10)

            VStack(alignment: .leading, spacing: 10) {
                Label(URL(string: url)?.host ?? url, systemImage: "link")
                    .font(.system(size: 12))
                    .foregroundStyle(Palette.inkDim)
                    .lineLimit(2)
                    .textSelection(.enabled)
                DisclosureGroup("Pairing details", isExpanded: $showingPairingDetails) {
                    VStack(alignment: .leading, spacing: 12) {
                        Field(label: "Server URL") {
                            TextField("Server URL", text: urlBinding)
                                .textFieldStyle(.plain)
                                .autocorrectionDisabled()
                                .focused($focus, equals: .url)
                        }
                        Field(label: "Pairing code") {
                            TextField("XXXX-XXXX", text: codeBinding)
                                .textFieldStyle(.plain)
                                .font(Typo.mono(12))
                                .autocorrectionDisabled()
                                .focused($focus, equals: .code)
                                .onSubmit { submit() }
                        }
                    }
                    .padding(.top, 10)
                    // A rejected code is corrected HERE, so the fields it
                    // lives in open the moment there is something to correct.
                    .onChange(of: pairError) { _, error in
                        if error != nil { showingPairingDetails = true }
                    }
                }
                .font(.system(size: 11))
                .foregroundStyle(Palette.inkFaint)
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 12).fill(Palette.canvas.opacity(0.3)))
        }
    }

    private var pairForm: some View {
        VStack(alignment: .leading, spacing: 14) {
            Field(label: "server url") {
                TextField(serverPlaceholder, text: urlBinding)
                    .textFieldStyle(.plain)
                    .textContentType(.URL)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .url)
                    .onSubmit { focus = .code }
            }
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 5) {
                    FieldLabel("pairing code")
                    if hostingChoice == .selfHosted {
                        Button { pairingHelp = true } label: {
                            Image(systemName: "questionmark.circle")
                                .font(.system(size: 11))
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(Palette.inkFaint)
                        .popover(isPresented: $pairingHelp) {
                            VStack(alignment: .leading, spacing: 10) {
                                Text("Run this on the machine hosting squelchd:")
                                    .font(.system(size: 12))
                                Text("squelchd pair")
                                    .font(Typo.mono(13))
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 7)
                                    .background(RoundedRectangle(cornerRadius: 8).fill(Palette.canvas))
                            }
                            .padding(16)
                        }
                    }
                }
                TextField("XXXX-XXXX", text: codeBinding)
                    .textFieldStyle(.plain)
                    // Monospaced so a code read off a terminal lines up with
                    // what the terminal showed, character for character.
                    .font(Typo.mono(13))
                    .autocorrectionDisabled()
                    .focused($focus, equals: .code)
                    .onSubmit { submit() }
                    .fieldWell()
            }
            Field(label: "device name") {
                TextField("this Mac", text: $deviceName)
                    .textFieldStyle(.plain)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .device)
                    .onSubmit { submit() }
            }
        }
    }

    private var serverPlaceholder: String {
        hostingChoice == .hosted
            ? "https://username.passband.email"
            : "http://127.0.0.1:8848"
    }

    private var tokenForm: some View {
        VStack(alignment: .leading, spacing: 14) {
            Field(label: "server url") {
                TextField("http://127.0.0.1:8848", text: urlBinding)
                    .textFieldStyle(.plain)
                    .textContentType(.URL)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .url)
                    .onSubmit { focus = .token }
            }
            Field(label: "api token") {
                SecureField("SQUELCH_API_TOKEN", text: $token)
                    .textFieldStyle(.plain)
                    .focused($focus, equals: .token)
                    .onSubmit { submit() }
            }
        }
    }

    private var buttonLabel: String {
        if claiming { return "pairing…" }
        if adding || store.connStatus == .connecting { return "testing…" }
        // The claim is done and only its probe is outstanding, so the button
        // offers the step that is actually left.
        let done = purpose == .gate ? "connect" : "add account"
        if mode == .pair && heldToken != nil { return done }
        return mode == .pair ? "pair" : done
    }

    /// The mode switch, written by hand so flipping it also clears the stale
    /// error from the other path. A raw `$mode` would leave "token rejected"
    /// sitting over the pairing form.
    private var modeBinding: Binding<ConnectMode> {
        Binding(
            get: { mode },
            set: { next in
                guard next != mode else { return }
                mode = next
                pairError = nil
                addError = nil
                store.connError = nil
                focus = next == .pair ? .code : .token
            })
    }

    /// The server-url field, hand-written because a typed edit has to drop a
    /// token held from an earlier claim: it belongs to the daemon that minted
    /// it, not to whatever host the field now names.
    private var urlBinding: Binding<String> {
        Binding(
            get: { url },
            set: { next in
                guard next != url else { return }
                url = next
                heldToken = nil
                linkArmed = false
                linkAid = nil
            })
    }

    /// The pairing-code field. A different code means a different claim, so the
    /// token the last one bought is no longer the thing a press should retry.
    private var codeBinding: Binding<String> {
        Binding(
            get: { code },
            set: { next in
                guard next != code else { return }
                code = next
                heldToken = nil
                linkArmed = false
                linkAid = nil
            })
    }

    // MARK: - actions

    private func submit() {
        guard canSubmit else { return }
        switch mode {
        case .pair: Task { await claim() }
        case .token: Task { await finish(serverURL: url.trimmed, apiToken: token.trimmed) }
        }
    }

    /// THE destination both forms share, and the one place `purpose` changes
    /// what happens: the gate's credentials become this install's identity, the
    /// sheet's become one more account beside it. Returns whether the
    /// credentials were accepted, which is what tells `claim` its held token is
    /// spent.
    @discardableResult
    private func finish(serverURL: String, apiToken: String) async -> Bool {
        addError = nil
        switch purpose {
        case .gate:
            return await store.connect(
                serverURL: serverURL, apiToken: apiToken, label: accountLabel.trimmed)
        case .addAccount:
            adding = true
            let outcome = await store.addAccount(
                serverURL: serverURL, apiToken: apiToken, label: accountLabel.trimmed)
            adding = false
            addError = outcome.error
            // The sheet's whole job is done, and the app behind it has already
            // switched to the account that was just added.
            if outcome.ok { dismiss() }
            return outcome.ok
        }
    }

    /// Claim the code, then hand the issued token to the SAME path a pasted one
    /// takes: `connect` proves it against /client/stats and stores it. Pairing
    /// adds a step in front of that flow, it does not replace it.
    ///
    /// A press AFTER the probe failed does not claim again. The token the first
    /// claim minted is held and re-probed, because a second claim would spend
    /// another of the code's five attempts and leave the first token orphaned
    /// server-side: minted, held by nobody, and only removable by hand.
    private func claim() async {
        let base = url.trimmed
        linkArmed = false
        pairError = nil
        addError = nil
        store.connError = nil

        // Already paid for. Retry the connection, not the claim.
        if let held = heldToken {
            if await finish(serverURL: base, apiToken: held) {
                heldToken = nil
                adoptLinkAid()
            }
            return
        }

        // A duplicate daemon must be refused BEFORE the claim, not after:
        // claiming mints a device token on the daemon and spends one of the
        // code's five attempts, and `addAccount`'s own check would only see
        // the duplicate once both are already gone.
        if purpose == .addAccount, store.isKnownDaemon(base) {
            addError = "that daemon is already one of your accounts"
            return
        }

        let typedCode = code
        let name = Pairing.clampDeviceName(deviceName)
        claiming = true
        do {
            let issued = try await Pairing.claim(baseURL: base, code: typedCode, deviceName: name)
            claiming = false
            // Spent the moment the daemon answers: a code is one-shot, and
            // leaving it on screen invites a second press that can only fail.
            // Assigned to the state directly rather than through `codeBinding`,
            // which is for USER edits and would drop the token we just held.
            code = ""
            heldToken = issued.token
            if await finish(serverURL: base, apiToken: issued.token) {
                heldToken = nil
                adoptLinkAid()
            }
        } catch {
            claiming = false
            pairError = Pairing.message(for: error)
        }
    }

    /// Adopt the deep link's analytics id, now that the pairing it came with has
    /// actually landed. Called from BOTH success paths in `claim`, because a
    /// connect that only succeeded on the second press is no less a pairing —
    /// the retry re-probes a token the same link's code already bought.
    ///
    /// Deliberately not reached from anywhere else. A token-mode connect and a
    /// hand-typed code carry no id, so there is nothing to adopt and nothing
    /// invented: that install stays anonymous until the next `/app/auth` sign-in
    /// link brings the id over, which heals it without a guess. Runs for the
    /// add-account sheet as well as the gate — `Analytics.adopt` decides what a
    /// second account means, and that decision belongs there rather than split
    /// across a view that only knows about one form.
    private func adoptLinkAid() {
        guard let aid = linkAid else { return }
        Analytics.adopt(analyticsId: aid)
        linkAid = nil
    }

    /// Fill the form from a deep link. It NEVER claims, whatever host it names:
    /// a `passband://` URL is openable by any web page, every claim spends one
    /// of the live code's five attempts, and a 200 from whatever answers the
    /// named port is a token this app would then store. So a link gets the user
    /// a filled form with the button ringed, and no further.
    private func applyPairLink(_ link: PairLink?) {
        guard let link else { return }
        // Busy means a claim, an add or a saved-connection retry is mid-
        // flight, and a form edited under it would be claimed against the
        // wrong host. The link stays PARKED on the store rather than consumed:
        // `onChange(of: busy)` applies it the moment the form is free.
        guard !busy else { return }
        store.pairLink = nil
        if purpose == .gate { gateStep = .credentials(nil) }
        mode = .pair
        url = link.serverURL
        code = Pairing.formatted(link.code)
        // Copied, not merged: a new link replaces the previous link's claim
        // whole, and a self-hosted one (which carries no id) correctly clears
        // whatever a hosted link left sitting here.
        linkAid = link.analyticsId
        heldToken = nil
        pairError = nil
        addError = nil
        store.connError = nil
        linkArmed = true
        arrivedViaPairLink = true
        showingPairingDetails = false
        // At the gate the form is the whole screen and Return is the natural
        // next act. The Add Account sheet is raised OVER whatever the human
        // was doing, and focusing submit there would let a Return already in
        // flight claim against the link's chosen host — pairing from a link
        // while connected takes an explicit click.
        focus = purpose == .gate ? .submit : nil
    }
}

/// One connection choice: hosted login/signup, with self-hosting secondary.
private struct WelcomeGate: View {
    @Environment(\.colorScheme) private var colorScheme
    /// Present only when the gate is up because a saved connection could
    /// not be restored: the retry belongs on this screen too, since back
    /// from the form lands here.
    var retrySaved: (() -> Void)? = nil
    var retrying = false
    let login: () -> Void
    let selfHostedLogin: () -> Void
    @State private var browserOpened = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Welcome to Passband.")
                .font(Typo.serif(40, weight: .medium))
                .foregroundStyle(Palette.ink)
            Text("Log in to your account or get started.")
                .font(.system(size: 15))
                .foregroundStyle(Palette.inkDim)
                .padding(.top, 8)

            HStack(spacing: 14) {
                RouteOption(symbol: "person.crop.circle", title: "Log in", action: login)
                RouteOption(symbol: "person.badge.plus", title: "Sign up") {
                    browserOpened = true
                    Opener.open(Hosted.signUp(theme: colorScheme))
                }
            }
            .padding(.top, 30)

            Button(action: selfHostedLogin) {
                Text("Self-hosted login")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(Palette.inkDim)
                    .frame(maxWidth: .infinity, minHeight: 42)
                    .contentShape(Rectangle())
            }
                .buttonStyle(.plain)
                .background(RoundedRectangle(cornerRadius: 12).fill(Palette.canvas.opacity(0.3)))
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Palette.hairline, lineWidth: 0.5))
                .padding(.top, 14)

            if browserOpened {
                Text("Continue in your browser. You’ll return here when you’re ready.")
                    .font(.system(size: 12))
                    .foregroundStyle(Palette.inkDim)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 10)
            }

            if let retrySaved {
                Button(retrying ? "Trying saved connection…" : "Try saved connection again", action: retrySaved)
                    .buttonStyle(.plain)
                    .font(.system(size: 12))
                    .foregroundStyle(Palette.inkDim)
                    .disabled(retrying)
                    .padding(.top, 18)
            }
        }
        .padding(34)
        #if os(macOS)
            .frame(width: 600)
        #else
            .frame(maxWidth: 600)
        #endif
        .passbandGlass(.chrome, cornerRadius: 24, tint: Palette.glassTint.opacity(0.35))
        .shadow(color: .black.opacity(0.3), radius: 50, y: 24)
    }
}

private struct RouteOption: View {
    let symbol: String
    let title: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 11) {
                Image(systemName: symbol)
                    .font(.system(size: 21, weight: .medium))
                    .foregroundStyle(Palette.accent)
                    .frame(width: 30)
                Text(title)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(Palette.ink)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity, minHeight: 34, alignment: .leading)
            .padding(.horizontal, 15)
            .padding(.vertical, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(RoundedRectangle(cornerRadius: 16).fill(Palette.canvas.opacity(0.62)))
        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(Palette.hairline, lineWidth: 0.75))
    }
}

/// Self-hosting stays inside Passband: the shortest complete path from no
/// daemon to a pairing code, with copyable commands and an explicit handoff to
/// the form once the server is running.
private struct SelfHostGuide: View {
    let back: () -> Void
    let continueToForm: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("set up your server")
                .font(Typo.serif(34, weight: .medium))
                .foregroundStyle(Palette.ink)
            Text("Run squelchd on this Mac, a NAS, or a server you control.")
                .font(.system(size: 13))
                .foregroundStyle(Palette.inkFaint)
                .padding(.top, 5)

            VStack(spacing: 12) {
                SetupStep(
                    number: 1,
                    title: "install squelchd",
                    detail: "Pull the public image for amd64 or arm64.",
                    command: "docker pull ghcr.io/braelyn-ai/squelchd")
                SetupStep(
                    number: 2,
                    title: "authorize gmail",
                    detail: "Complete Google's one-time consent on the server.",
                    command: "squelchd auth")
                SetupStep(
                    number: 3,
                    title: "pair this device",
                    detail: "Mint the short code the next screen accepts.",
                    command: "squelchd pair")
            }
            .padding(.top, 22)

            Button("view detailed instructions on GitHub") {
                Opener.open(
                    "https://github.com/braelyn-ai/squelch/blob/main/docs/GETTING-STARTED.md")
            }
            .buttonStyle(.textAction)
            .padding(.top, 16)

            HStack {
                Button("back", action: back)
                Spacer()
                Button("my server is running", action: continueToForm)
                    .buttonStyle(.borderedProminent)
                    .tint(Palette.accent)
            }
            .padding(.top, 22)
        }
        .padding(34)
        #if os(macOS)
            .frame(width: 620)
        #else
            .frame(maxWidth: 620)
        #endif
        .passbandGlass(.chrome, cornerRadius: 24, tint: Palette.glassTint.opacity(0.35))
        .shadow(color: .black.opacity(0.3), radius: 50, y: 24)
    }
}

private struct SetupStep: View {
    let number: Int
    let title: String
    let detail: String
    let command: String
    @State private var copied = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(number)")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Palette.accentInk)
                .frame(width: 23, height: 23)
                .background(Circle().fill(Palette.accent))
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 13, weight: .semibold))
                Text(detail)
                    .font(.system(size: 11))
                    .foregroundStyle(Palette.inkFaint)
                HStack(spacing: 8) {
                    Text(command)
                        .font(Typo.mono(11))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 4)
                    Button {
                        Clip.copy(command, flashing: $copied)
                    } label: {
                        Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    }
                    .buttonStyle(.plain)
                }
                .padding(.horizontal, 9)
                .padding(.vertical, 6)
                .background(RoundedRectangle(cornerRadius: 8).fill(Palette.canvas.opacity(0.65)))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Palette.hairline, lineWidth: 0.75))
                .padding(.top, 2)
            }
        }
    }
}

/// Always reachable during first-run setup, before Settings exists as a
/// destination. It uses the same persisted preference and flip semantics as
/// the app-wide keyboard shortcut.
private struct SetupThemeToggle: View {
    @Environment(Prefs.self) private var prefs

    private var isDark: Bool {
        switch prefs.theme {
        case .dark: true
        case .light: false
        case .system: Platform.isDarkAppearance
        }
    }

    var body: some View {
        Button { prefs.flipTheme() } label: {
            Image(systemName: isDark ? "sun.max.fill" : "moon.fill")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Palette.ink)
                .frame(width: 30, height: 30)
        }
        .buttonStyle(.plain)
        .glassCapsule(tint: Palette.glassTint.opacity(0.35))
        .help(isDark ? "use light mode" : "use dark mode")
        .accessibilityLabel(isDark ? "Use light mode" : "Use dark mode")
    }
}

/// A labelled input well. Near-opaque on purpose: a fully translucent field
/// over a busy wallpaper leaves typed text unreadable.
struct Field<Content: View>: View {
    let label: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            FieldLabel(label)
            content.fieldWell()
        }
    }
}

/// The caption above a well. Split out so a field needing its own row under the
/// label still labels itself identically.
struct FieldLabel: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(Typo.micro)
            .foregroundStyle(Palette.inkFaint)
            .textCase(.lowercase)
    }
}

extension View {
    func fieldWell() -> some View {
        self
            .font(.system(size: 13))
            .foregroundStyle(Palette.ink)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(Palette.canvas.opacity(0.65))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .strokeBorder(Palette.hairlineStrong, lineWidth: 0.75)
            )
    }
}

extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
