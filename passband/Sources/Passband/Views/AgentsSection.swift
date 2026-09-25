// Settings' Agents pane: connecting an agent the human already runs (OpenClaw,
// Claude Code, Codex…) to this mailbox over MCP.
//
// Three cards, in the order somebody needs them: WHERE the door is and whether
// it is open, HOW to point a given agent at it, and WHAT the agent gets once it
// is through. The last one is not decoration. Handing an agent your mail is
// the kind of decision people make once and remember, and the honest version
// of it includes the parts the agent cannot do and the fact that the door has
// no lock of its own.
//
// Shared by both shells: the phone files it under the account page, the Mac
// beside the mail. This is also where managing connected agents will live
// once the daemon can name them; today it can only say whether the door is
// open, so that is all it claims.

import SwiftUI

struct AgentsSection: View {
    @Environment(AppStore.self) private var store
    @State private var connect = AgentConnect.shared
    @State private var agent: AgentKind = .openClaw

    private var endpoint: String? { AgentConnect.endpoint(serverURL: store.settings?.serverURL) }
    private var status: AgentDoorStatus { connect.status(for: endpoint) }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            doorCard
            if let endpoint {
                setupCard(endpoint)
            }
            reachCard
        }
        .task(id: endpoint) {
            await connect.check(serverURL: store.settings?.serverURL)
        }
    }

    // MARK: - where the door is

    private var doorCard: some View {
        SectionCard(label: "Connect an agent") {
            SettingsHint(
                "Point an agent you already run at your mailbox. It reads what Passband has triaged, over MCP, and it never holds your Gmail sign-in."
            )
            if let endpoint {
                InlineRow(key: "endpoint", alignment: .top) {
                    VStack(alignment: .leading, spacing: 7) {
                        HStack(spacing: 8) {
                            Text(endpoint)
                                .font(Typo.mono(12))
                                .foregroundStyle(Palette.ink)
                                .textSelection(.enabled)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            CopyButton(text: endpoint)
                        }
                        HStack(spacing: 10) {
                            statusDot
                            if status != .checking {
                                Button("check again") {
                                    Task { await connect.check(serverURL: store.settings?.serverURL) }
                                }
                                .buttonStyle(.plain)
                                .font(Typo.micro)
                                .foregroundStyle(Palette.accent)
                            }
                        }
                    }
                }
                statusExplanation(endpoint)
            } else {
                SettingsHint("Connect to a server under General first. The agent door lives on the same one.")
            }
        }
    }

    @ViewBuilder private var statusDot: some View {
        switch status {
        case .unknown, .checking:
            StatusDot(color: Palette.inkFaintest, label: "checking…")
        case .open:
            StatusDot(color: Palette.positive, label: "open")
        case .hostRefused:
            StatusDot(color: Palette.warn, label: "refused by name")
        case .notServed:
            StatusDot(color: Palette.inkFaint, label: "not offered here")
        case .unreachable:
            StatusDot(color: Palette.danger, label: "no answer")
        }
    }

    @ViewBuilder private func statusExplanation(_ endpoint: String) -> some View {
        switch status {
        case .open where AgentConnect.isLoopback(endpoint):
            SettingsHint(
                "This address only works on this computer, so the agent has to run here too. For an agent on another machine, reach the daemon over your tailnet instead."
            )
        case .open:
            SettingsHint("Any agent that can reach this address can connect.")
        case .hostRefused:
            let host = AgentConnect.host(of: endpoint) ?? "your-host"
            let line = "SQUELCH_MCP_ALLOWED_HOSTS=\(host)"
            SettingsHint(
                "Your daemon is up, but it only answers agents that call it localhost. Add this to the daemon's environment and restart it:"
            )
            CodeBlock(text: line)
        case .notServed:
            SettingsHint(
                "Nothing answers at /mcp on this server. Hosted Passband doesn't open the agent door yet. A self-hosted daemon does."
            )
        case .unreachable:
            SettingsHint(
                "The agent door didn't answer from here. If the daemon is on another machine, check it is running and that this address reaches it."
            )
        case .unknown, .checking:
            EmptyView()
        }
    }

    // MARK: - how to connect one

    private func setupCard(_ endpoint: String) -> some View {
        SectionCard(label: "Set it up", note: agent.name) {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 142), spacing: 8)], spacing: 8) {
                ForEach(AgentKind.allCases) { kind in
                    agentChip(kind)
                }
            }
            SettingsHint(agent.instruction)
            CodeBlock(text: agent.snippet(endpoint: endpoint))
            if let check = agent.check {
                HStack(spacing: 8) {
                    Text("then check with")
                        .font(Typo.micro)
                        .foregroundStyle(Palette.inkFaintest)
                    Text(check)
                        .font(Typo.mono(10))
                        .foregroundStyle(Palette.inkDim)
                        .textSelection(.enabled)
                }
            }
        }
    }

    private func agentChip(_ kind: AgentKind) -> some View {
        let active = kind == agent
        return Button {
            withAnimation(Motion.disclose) { agent = kind }
        } label: {
            HStack(spacing: 7) {
                Image(systemName: kind.symbol)
                    .font(.system(size: 11, weight: .medium))
                    .frame(width: 14)
                Text(kind.name)
                    .font(.system(size: 12, weight: .medium))
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            .foregroundStyle(active ? Palette.accent : Palette.inkDim)
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(active ? Palette.accentSoft : Palette.canvas.opacity(0.5))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .strokeBorder(active ? Palette.accent.opacity(0.4) : Palette.hairline, lineWidth: 0.75)
        )
        .accessibilityAddTraits(active ? .isSelected : [])
    }

    // MARK: - what it gets

    private var reachCard: some View {
        SectionCard(label: "What your agent can do") {
            HStack(alignment: .top, spacing: 18) {
                reachColumn(
                    "can", symbol: "checkmark", tint: Palette.positive,
                    items: [
                        "Read ranked updates and whole threads",
                        "Search your mail",
                        "List bills, deadlines and packages",
                        "Set sender rules, inside Passband only",
                    ])
                reachColumn(
                    "can't", symbol: "xmark", tint: Palette.danger,
                    items: [
                        "Send, archive, label or delete",
                        "See sign-in codes or password resets",
                        "Change anything in Gmail",
                    ])
            }
            SettingsHint(
                "The agent door has no password of its own. Anything that can reach the address above can read your triaged mail, so keep it on this computer or your tailnet."
            )
        }
    }

    private func reachColumn(_ title: String, symbol: String, tint: Color, items: [String]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(Typo.micro)
                .foregroundStyle(Palette.inkFaint)
                .textCase(.uppercase)
            ForEach(items, id: \.self) { item in
                HStack(alignment: .firstTextBaseline, spacing: 7) {
                    Image(systemName: symbol)
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(tint)
                        .frame(width: 11)
                    Text(item)
                        .font(Typo.rowSub)
                        .foregroundStyle(Palette.inkDim)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A command or a config fragment, selectable, with its own copy button. The
/// button copies exactly what is drawn, so there is no second string to drift.
private struct CodeBlock: View {
    let text: String

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text)
                    .font(Typo.mono(11))
                    .foregroundStyle(Palette.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: true)
                    .padding(.vertical, 1)
            }
            CopyButton(text: text)
        }
        .padding(.horizontal, 11)
        .padding(.vertical, 9)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(Palette.canvas.opacity(0.75))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .strokeBorder(Palette.hairlineStrong, lineWidth: 0.75)
        )
    }
}

private struct CopyButton: View {
    let text: String
    @State private var copied = false

    var body: some View {
        Button {
            Clip.copy(text, flashing: $copied)
        } label: {
            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(copied ? Palette.positive : Palette.inkFaint)
                .frame(width: 18, height: 18)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(copied ? "Copied" : "Copy")
        .accessibilityLabel(copied ? "Copied" : "Copy")
    }
}
