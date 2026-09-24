import SwiftUI

/// A user-paced introduction, shared by first connection and the isolated
/// rehearsal. The illustrated messages are examples, never account data.
struct OnboardingIntroView: View {
    let onContinue: () -> Void
    var continueTitle = "Bring your inbox"
    var continueHint = "Continue to account connection"

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var colorScheme
    @State private var page = 0

    private var organized: Bool { page > 0 }

    var body: some View {
        if SquelchSceneView.renderer != nil {
            squelchLayout
        } else {
            cardLayout
        }
    }

    /// The radio beats: raw noise pours through a dormant gate, then the
    /// continue button closes the squelch and only the passband comes out.
    /// Always dark; glowing lines are the whole picture.
    private var squelchLayout: some View {
        GeometryReader { geometry in
            let wide = geometry.size.width >= 820
            ZStack(alignment: .topLeading) {
                SquelchSceneView(engaged: organized)
                    .ignoresSafeArea()
                LinearGradient(
                    stops: [
                        .init(color: IntroNight.backdrop.opacity(0.92), location: 0),
                        .init(color: IntroNight.backdrop.opacity(0.6), location: wide ? 0.32 : 0.4),
                        .init(color: .clear, location: wide ? 0.62 : 0.75),
                    ],
                    startPoint: wide ? .leading : .top,
                    endPoint: wide ? .trailing : .bottom)
                    .ignoresSafeArea()
                    .allowsHitTesting(false)
                // Keeps the controls legible over the nearest, brightest bands.
                LinearGradient(
                    colors: [.clear, IntroNight.backdrop.opacity(0.85)],
                    startPoint: UnitPoint(x: 0.5, y: 0.72), endPoint: .bottom)
                    .ignoresSafeArea()
                    .allowsHitTesting(false)
                VStack(alignment: .leading, spacing: 0) {
                    masthead
                        .modifier(IntroEntrance(delay: 0, distance: 4))
                    Spacer(minLength: 24)
                    story(wide: wide)
                        .frame(maxWidth: wide ? 480 : .infinity, alignment: .leading)
                    Spacer(minLength: wide ? 24 : 260)
                    controls
                }
                .padding(wide ? 44 : 24)
            }
        }
        .environment(\.colorScheme, .dark)
    }

    /// The fallback when Metal is unavailable: illustrated example mail.
    private var cardLayout: some View {
        GeometryReader { geometry in
            let wide = geometry.size.width >= 820
            ScrollView {
                VStack(alignment: .leading, spacing: wide ? 40 : 24) {
                    masthead
                        .modifier(IntroEntrance(delay: 0, distance: 4))

                    if wide {
                        HStack(alignment: .center, spacing: 44) {
                            story(wide: true)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            mailIllustration
                                .frame(width: 380, height: 380)
                        }
                    } else {
                        story(wide: false)
                        mailIllustration
                            .frame(maxWidth: 440)
                            .frame(height: 290)
                            .frame(maxWidth: .infinity)
                    }

                    controls
                }
                .padding(wide ? 44 : 24)
                .frame(maxWidth: 1060)
                .frame(minHeight: geometry.size.height, alignment: .center)
                .frame(maxWidth: .infinity)
            }
        }
        .background {
            RadialGradient(
                colors: [Palette.accentSoft.opacity(0.65), .clear],
                center: .trailing, startRadius: 10, endRadius: 650)
                .allowsHitTesting(false)
        }
    }

    private var masthead: some View {
        HStack {
            HStack(spacing: 9) {
                if let mark = colorScheme == .dark ? IntroBrand.lightMark : IntroBrand.mark {
                    mark.resizable()
                        .scaledToFit()
                        .frame(width: 42, height: 24)
                        .accessibilityHidden(true)
                }
                Text("passband")
                    .font(Typo.serif(24, weight: .medium))
            }
            .foregroundStyle(Palette.ink)
            Spacer()
            if RehearsalMode.launchedStandalone {
                DemoAppearancePicker()
                    .padding(.trailing, 12)
            }
        }
    }

    private func story(wide: Bool) -> some View {
        VStack(alignment: .leading, spacing: 22) {
            VStack(alignment: .leading, spacing: 18) {
                if organized {
                    Text("Know what needs you.")
                        .font(Typo.hero(wide ? 49 : 36))
                        .modifier(IntroEntrance(delay: 0.04))
                } else {
                    Text("Inbox zero every day was never realistic.")
                        .font(Typo.serif(wide ? 33 : 27))
                        .foregroundStyle(Palette.inkDim)
                        .modifier(IntroEntrance(delay: 0.04))
                    Text("You’re only human.")
                        .font(Typo.hero(wide ? 49 : 36))
                        .modifier(IntroEntrance(delay: 0.14))
                }
            }
            .foregroundStyle(Palette.ink)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.isHeader)

            Text(organized
                 ? "Passband brings the important things forward, so you can give them your attention and get on with your day."
                 : "Your attention is valuable. You deserve an inbox that treats it that way.")
                .font(.system(size: 15))
                .foregroundStyle(Palette.inkDim)
                .lineSpacing(5)
                .fixedSize(horizontal: false, vertical: true)
                .modifier(IntroEntrance(delay: organized ? 0.16 : 0.24))
        }
        // Recreate just the copy on a new beat, so it settles in once while
        // the existing mail cards continue into their new positions.
        .id(page)
    }

    private var mailIllustration: some View {
        GeometryReader { geometry in
            let width = min(geometry.size.width - 24, 330.0)
            let centerX = geometry.size.width / 2
            let centerY = geometry.size.height / 2 - 8

            ZStack {
                Circle()
                    .fill(Palette.accentSoft.opacity(0.4))
                    .frame(width: min(geometry.size.width, 340))
                    .overlay(Circle().strokeBorder(Palette.hairline, lineWidth: 0.75))
                    .position(x: centerX, y: centerY)

                ForEach(Array(IntroMail.examples.enumerated()), id: \.element.id) { index, mail in
                    IntroMailCard(mail: mail, organized: organized)
                        .frame(width: width)
                        .rotationEffect(.degrees(organized ? 0 : Double(index - 1) * 9))
                        .scaleEffect(organized ? (index == 0 ? 1 : 0.94) : 1 - Double(index) * 0.035)
                        .position(
                            x: centerX + (organized ? 0 : CGFloat(index - 1) * 9),
                            y: centerY + (organized ? CGFloat(index - 1) * 89 : CGFloat(index - 1) * 29))
                        .animation(
                            reduceMotion ? nil : .easeInOut(duration: 0.65).delay(Double(index) * 0.045),
                            value: organized)
                        .modifier(IntroEntrance(delay: 0.10 + Double(index) * 0.07, distance: 10))
                        .zIndex(Double(3 - index))
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(organized
            ? "Example mail organized: a request needs you, a receipt is saved in records, and a newsletter is set aside for later."
            : "An overlapping pile of example emails: a request, a receipt, and a newsletter.")
    }

    private var controls: some View {
        HStack(spacing: 16) {
            HStack(spacing: 6) {
                ForEach(0..<2) { index in
                    Capsule().fill(page == index ? Palette.accent : Palette.hairlineStrong)
                        .frame(width: page == index ? 24 : 8, height: 5)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Introduction, step \(page + 1) of 2")
            .animation(reduceMotion ? nil : .easeInOut(duration: 0.28), value: page)

            if organized {
                Button("Back") { page -= 1 }
                    .buttonStyle(.plain)
                    .foregroundStyle(Palette.inkDim)
            }

            Spacer(minLength: 0)
            Button("Skip intro", action: onContinue)
                .buttonStyle(.plain)
                .font(.system(size: 11))
                .foregroundStyle(Palette.inkFaintest)
                .padding(.vertical, 8)
                .accessibilityHint(continueHint)
            Button {
                if page == 1 { onContinue() }
                else { page += 1 }
            } label: {
                HStack(spacing: 9) {
                    Text(organized ? continueTitle : "Find a little breathing room")
                    Image(systemName: "arrow.right")
                }
                .font(.system(size: 13, weight: .semibold))
                .padding(.vertical, 7)
                .padding(.horizontal, 8)
            }
            .buttonStyle(.borderedProminent)
            .tint(Palette.accent)
            .keyboardShortcut(.defaultAction)
        }
        .padding(.bottom, 24)
    }
}

/// The scene's own backdrop color (the shader's top gradient stop), so the
/// scrim behind the copy melts into it rather than tinting it.
private enum IntroNight {
    static let backdrop = Color(red: 0.035, green: 0.05, blue: 0.085)
}

/// A single quiet entrance per view identity. No timers or repeating motion;
/// leaving a beat cancels its task, and Reduce Motion renders it immediately.
private struct IntroEntrance: ViewModifier {
    let delay: Double
    var distance: CGFloat = 7

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var appeared = false

    func body(content: Content) -> some View {
        content
            .opacity(reduceMotion || appeared ? 1 : 0)
            .offset(y: reduceMotion || appeared ? 0 : distance)
            .animation(reduceMotion ? nil : .easeOut(duration: 0.48).delay(delay), value: appeared)
            .task {
                await Task.yield()
                guard !Task.isCancelled else { return }
                appeared = true
            }
    }
}

/// Verbatim transparent exports from brand/png, with ink and lit variants.
/// Native image decoding works in the CLI Mac bundle and the iOS bundle.
@MainActor
private enum IntroBrand {
    static let mark = load("passband-mark")
    static let lightMark = load("passband-mark-light")

    private static func load(_ name: String) -> Image? {
        let url = Bundle.main.url(forResource: name, withExtension: "png", subdirectory: "Brand")
            ?? Bundle.main.url(forResource: name, withExtension: "png", subdirectory: "Resources/Brand")
        guard let url, let data = try? Data(contentsOf: url),
              let nativeImage = PlatformImage(data: data) else { return nil }
        return Image(platformImage: nativeImage)
    }
}

private struct IntroMail: Identifiable {
    let id: Int
    let sender: String
    let subject: String
    let destination: String
    let symbol: String
    let color: Color

    static let examples: [IntroMail] = [
        .init(id: 0, sender: "Jamie", subject: "A quick decision before Friday",
              destination: "Needs you", symbol: "arrow.turn.up.left", color: Palette.warn),
        .init(id: 1, sender: "Your neighborhood café", subject: "Your receipt. Thanks for stopping by.",
              destination: "Receipts", symbol: "tray", color: Palette.inkDim),
        .init(id: 2, sender: "The Sunday Edit", subject: "A few things we thought you’d like",
              destination: "For later", symbol: "book", color: Palette.inkDim),
    ]
}

private struct IntroMailCard: View {
    let mail: IntroMail
    let organized: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: organized ? mail.symbol : "envelope")
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(organized ? mail.color : Palette.inkDim)
                .frame(width: 35, height: 35)
                .background((organized ? mail.color : Palette.inkDim).opacity(0.09), in: RoundedRectangle(cornerRadius: 11))
            VStack(alignment: .leading, spacing: 5) {
                HStack {
                    Text(mail.sender)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Palette.ink)
                    Spacer(minLength: 4)
                    if organized {
                        Text(mail.destination)
                            .font(.system(size: 10, weight: .medium))
                            .foregroundStyle(mail.color)
                    }
                }
                Text(mail.subject)
                    .font(.system(size: 11))
                    .foregroundStyle(Palette.inkDim)
                    .lineLimit(1)
            }
        }
        .padding(16)
        .frame(height: 76)
        .background(Color(light: 0xF7FAFD, dark: 0x293747), in: RoundedRectangle(cornerRadius: 17))
        .overlay(RoundedRectangle(cornerRadius: 17).strokeBorder(Palette.hairlineStrong, lineWidth: 0.75))
        .shadow(color: .black.opacity(organized ? 0.07 : 0.12), radius: organized ? 12 : 20, y: 8)
    }
}
