// The "connect Passband with your agent" corner card. Bottom RIGHT, because
// that is the corner nobody else owns on the board: toasts hold bottom-left,
// the update card bottom-centre, and the Settings version stamp only exists
// on Settings, where this card never shows.
//
// Not modal, for the same reason the update card is not: it is an offer, not
// a question the app needs answered before you can read your mail. It asks
// once (AgentConnect.dismissNudge), and only when the probe has seen the agent
// door open, so the button never leads to a pane explaining why it cannot work.

import SwiftUI

struct AgentConnectNudge: View {
    @Environment(AppStore.self) private var store
    @Environment(Prefs.self) private var prefs

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            AgentLinkArt()
                .frame(height: 74)
                .frame(maxWidth: .infinity)
                .background(
                    LinearGradient(
                        colors: [Palette.accentSoft, Palette.accentSoft.opacity(0.25)],
                        startPoint: .topLeading, endPoint: .bottomTrailing)
                )
                .clipShape(UnevenRoundedRectangle(
                    topLeadingRadius: 12, topTrailingRadius: 12, style: .continuous))
                .overlay(alignment: .topTrailing) { closeButton }

            VStack(alignment: .leading, spacing: 4) {
                Text("Connect Passband with your agent")
                    .font(Typo.serif(17, weight: .medium))
                    .foregroundStyle(Palette.ink)
                Text("Let OpenClaw, Claude Code or any MCP agent read your triaged inbox. It can't send or delete a thing.")
                    .font(Typo.rowSub)
                    .foregroundStyle(Palette.inkFaint)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)

            HStack(spacing: 8) {
                Spacer(minLength: 0)
                Button("Not now") { AgentConnect.shared.dismissNudge() }
                    .buttonStyle(.glass)
                    .font(.system(size: 12, weight: .medium))
                Button("Set up") { openSetup() }
                    .buttonStyle(.glassProminent)
                    .font(.system(size: 12, weight: .semibold))
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 13)
        }
        .frame(width: 304)
        .padding(4)
        .passbandGlass(.pane, cornerRadius: 16, tint: Palette.glassTintStrong)
        .transition(.move(edge: .trailing).combined(with: .opacity))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Connect Passband with your agent")
    }

    private var closeButton: some View {
        Button { AgentConnect.shared.dismissNudge() } label: {
            Image(systemName: "xmark")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Palette.inkFaint)
                .frame(width: 20, height: 20)
                .background(Circle().fill(Palette.canvas.opacity(0.7)))
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .padding(8)
        .help("Dismiss")
        .accessibilityLabel("Dismiss")
    }

    private func openSetup() {
        prefs.settingsSection = .agents
        store.setView(.settings)
        AgentConnect.shared.dismissNudge()
    }
}

/// The picture: the Passband mark, a dashed line with mail running along it,
/// and an agent on the far end whose face keeps changing, because "your agent"
/// means whichever one you run. Still under Reduce Motion.
private struct AgentLinkArt: View {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// The agents the far node cycles through. `other` is the grid's catch-all,
    /// not a face.
    private static let faces = AgentKind.allCases.filter { $0 != .other }

    var body: some View {
        HStack(spacing: 0) {
            node {
                if let mark = colorScheme == .dark ? IntroBrand.lightMark : IntroBrand.mark {
                    mark.resizable().scaledToFit().frame(width: 28, height: 16)
                } else {
                    Image(systemName: "envelope.fill")
                        .font(.system(size: 16, weight: .medium))
                        .foregroundStyle(Palette.accent)
                }
            }
            wire
                .frame(width: 118, height: 30)
            node { agentFace }
        }
    }

    private func node(@ViewBuilder _ content: () -> some View) -> some View {
        content()
            .frame(width: 46, height: 46)
            .background(
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .fill(Palette.canvas)
                    .shadow(color: Palette.accent.opacity(0.18), radius: 6, y: 2)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .strokeBorder(Palette.hairlineStrong, lineWidth: 0.75)
            )
    }

    @ViewBuilder private var agentFace: some View {
        if reduceMotion {
            Image(systemName: "sparkles")
                .font(.system(size: 17, weight: .medium))
                .foregroundStyle(Palette.accent)
        } else {
            TimelineView(.periodic(from: .now, by: 1.8)) { context in
                let step = Int(context.date.timeIntervalSinceReferenceDate / 1.8)
                let face = Self.faces[step % Self.faces.count]
                Image(systemName: face.symbol)
                    .font(.system(size: 17, weight: .medium))
                    .foregroundStyle(Palette.accent)
                    .contentTransition(.symbolEffect(.replace))
                    .animation(.smooth(duration: 0.4), value: face)
            }
        }
    }

    /// The dashed line and the mail on it. One Canvas, driven by the frame
    /// clock, so the dashes crawl and the envelope rides them at the same pace.
    @ViewBuilder private var wire: some View {
        if reduceMotion {
            wireCanvas(time: 0, moving: false)
        } else {
            TimelineView(.animation) { context in
                wireCanvas(time: context.date.timeIntervalSinceReferenceDate, moving: true)
            }
        }
    }

    private func wireCanvas(time: Double, moving: Bool) -> some View {
        Canvas { ctx, size in
            let y = size.height / 2
            let inset: CGFloat = 6
            var line = Path()
            line.move(to: CGPoint(x: inset, y: y))
            line.addLine(to: CGPoint(x: size.width - inset, y: y))
            let phase = CGFloat(-(time * 16).truncatingRemainder(dividingBy: 10))
            ctx.stroke(
                line, with: .color(Palette.accent.opacity(0.45)),
                style: StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [4, 6], dashPhase: phase))

            // End caps: where the wire plugs in.
            for x in [inset, size.width - inset] {
                ctx.fill(
                    Path(ellipseIn: CGRect(x: x - 2.5, y: y - 2.5, width: 5, height: 5)),
                    with: .color(Palette.accent))
            }

            // The letter in transit. Eased so it leaves and arrives rather than
            // sliding at one speed, and faded at both ends so it never pops.
            let period = 2.4
            let t = moving ? (time.truncatingRemainder(dividingBy: period)) / period : 0.5
            let eased = t < 0.5 ? 2 * t * t : 1 - pow(-2 * t + 2, 2) / 2
            let x = inset + 8 + CGFloat(eased) * (size.width - 2 * inset - 16)
            let fade = moving ? min(1, min(t, 1 - t) * 6) : 1
            if let letter = ctx.resolveSymbol(id: 0) {
                ctx.opacity = fade
                ctx.draw(letter, at: CGPoint(x: x, y: y))
            }
        } symbols: {
            Image(systemName: "envelope.fill")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Palette.accent)
                .padding(3)
                .background(Circle().fill(Palette.canvas))
                .tag(0)
        }
        .accessibilityHidden(true)
    }
}
