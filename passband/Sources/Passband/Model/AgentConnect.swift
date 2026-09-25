// Connecting an outside agent to the mailbox: where the agent door is, whether
// it answers, and what each agent wants pasted into it.
//
// THE AGENT DOOR IS NOT THE APP'S DOOR. This app talks to `/client/*` with a
// bearer token; an agent talks to `/mcp` with nothing at all. The daemon's only
// guard on `/mcp` is the Host header (loopback, plus SQUELCH_MCP_ALLOWED_HOSTS),
// and a hosted tenant's Ingress does not publish the path in the first place.
// So "is the agent door open" is a question only a request can answer, and the
// probe below asks it the way an agent would: an MCP `initialize`, with NO
// Authorization header, because an agent will not have one either.
//
// Settings' Agents pane reads `AgentConnect.shared`. It is a shared object
// rather than view state so the answer outlives leaving the pane.

import Foundation
import Observation

/// What a probe of `<server>/mcp` found.
enum AgentDoorStatus: Equatable, Sendable {
    /// Not asked yet, or not askable (no server, the practice inbox).
    case unknown
    case checking
    /// Answered an MCP `initialize`. An agent that can reach this address can
    /// connect.
    case open
    /// 403: the daemon is there and refused the Host name. It answers agents
    /// only by the names in SQUELCH_MCP_ALLOWED_HOSTS (and localhost).
    case hostRefused
    /// 404/410: nothing is mounted at `/mcp` here. What a hosted mailbox says
    /// today, since its Ingress publishes the human door only.
    case notServed
    /// No answer, or one this code does not recognise.
    case unreachable
}

/// An agent somebody might want to connect, and how that agent spells "add an
/// MCP server". Every snippet is the agent's own documented form, with the
/// daemon's real address already in it: a snippet with a placeholder is a
/// snippet somebody pastes with the placeholder still in.
enum AgentKind: String, CaseIterable, Identifiable, Sendable {
    case openClaw, claudeCode, codex, cursor, claudeDesktop, other

    var id: String { rawValue }

    var name: String {
        switch self {
        case .openClaw: "OpenClaw"
        case .claudeCode: "Claude Code"
        case .codex: "Codex"
        case .cursor: "Cursor"
        case .claudeDesktop: "Claude Desktop"
        case .other: "Other"
        }
    }

    var symbol: String {
        switch self {
        case .openClaw: "pawprint"
        case .claudeCode: "terminal"
        case .codex: "chevron.left.forwardslash.chevron.right"
        case .cursor: "cursorarrow.rays"
        case .claudeDesktop: "macwindow"
        case .other: "puzzlepiece.extension"
        }
    }

    /// Where the snippet goes, said before the snippet.
    var instruction: String {
        switch self {
        case .openClaw: "Run this on the machine your OpenClaw gateway runs on."
        case .claudeCode: "Run this in a terminal. Add --scope user to have it in every project."
        case .codex: "Run this in a terminal. The CLI and the IDE extension share it."
        case .cursor: "Add this to ~/.cursor/mcp.json, or to .cursor/mcp.json in one project."
        case .claudeDesktop:
            "Add this to claude_desktop_config.json (Settings, Developer, Edit Config), then restart Claude. It needs Node for npx."
        case .other: "Most MCP clients take a streamable HTTP server in this shape."
        }
    }

    /// How to see it worked, when the agent has a way to say so.
    var check: String? {
        switch self {
        case .openClaw: "openclaw mcp doctor passband --probe"
        case .claudeCode: "claude mcp list"
        case .codex: "codex mcp list"
        default: nil
        }
    }

    /// Whether `snippet` is a shell command (one line, run it) or config
    /// (a file fragment, paste it).
    var isCommand: Bool {
        switch self {
        case .openClaw, .claudeCode, .codex: true
        case .cursor, .claudeDesktop, .other: false
        }
    }

    func snippet(endpoint url: String) -> String {
        switch self {
        case .openClaw:
            return "openclaw mcp add passband --url \(url) --transport streamable-http"
        case .claudeCode:
            return "claude mcp add --transport http passband \(url)"
        case .codex:
            return "codex mcp add passband --url \(url)"
        case .cursor:
            return """
                {
                  "mcpServers": {
                    "passband": { "url": "\(url)" }
                  }
                }
                """
        case .claudeDesktop:
            // Claude Desktop's config file launches stdio servers only, so a
            // remote one goes through the mcp-remote bridge. The bridge refuses
            // plain http to anything but localhost unless told otherwise.
            var args = ["\"-y\"", "\"mcp-remote\"", "\"\(url)\""]
            if AgentConnect.isPlainHTTPToRemoteHost(url) { args.append("\"--allow-http\"") }
            return """
                {
                  "mcpServers": {
                    "passband": {
                      "command": "npx",
                      "args": [\(args.joined(separator: ", "))]
                    }
                  }
                }
                """
        case .other:
            return """
                {
                  "mcpServers": {
                    "passband": { "type": "http", "url": "\(url)" }
                  }
                }
                """
        }
    }
}

@MainActor
@Observable
final class AgentConnect {
    static let shared = AgentConnect()

    /// The last probe's answer, and the server it was asked of. Keyed so that
    /// switching accounts cannot show one daemon's answer under another's URL.
    private(set) var status: AgentDoorStatus = .unknown
    private(set) var statusFor: String?

    /// Bumped by every check, so only the newest one may write its answer.
    /// Keying on the endpoint alone is not enough: A, then B, then A again
    /// leaves the first A probe looking current when it lands.
    private var generation = 0

    // MARK: - the endpoint

    /// `<server>/mcp`, from the address this app already talks to. Nil with no
    /// server configured.
    nonisolated static func endpoint(serverURL: String?) -> String? {
        guard var base = serverURL?.trimmingCharacters(in: .whitespacesAndNewlines),
            !base.isEmpty
        else { return nil }
        while base.hasSuffix("/") { base.removeLast() }
        return base + "/mcp"
    }

    /// The host an agent will put in its Host header, which is what the daemon
    /// checks against SQUELCH_MCP_ALLOWED_HOSTS.
    nonisolated static func host(of url: String) -> String? {
        URLComponents(string: url)?.host
    }

    nonisolated static func isLoopback(_ url: String) -> Bool {
        guard let host = host(of: url)?.lowercased() else { return false }
        return host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]"
    }

    /// Whether mcp-remote needs `--allow-http` for this URL. Its exemption is
    /// narrower than `isLoopback`: only `localhost` and `127.0.0.1`, so an
    /// `http://[::1]` address still needs the flag.
    nonisolated static func isPlainHTTPToRemoteHost(_ url: String) -> Bool {
        guard url.lowercased().hasPrefix("http://") else { return false }
        let host = host(of: url)?.lowercased()
        return host != "localhost" && host != "127.0.0.1"
    }

    // MARK: - the probe

    /// Ask the door. Safe to call repeatedly; the newest call's answer wins.
    func check(serverURL: String?) async {
        generation &+= 1
        let mine = generation
        guard !RehearsalMode.isEnabled, let endpoint = Self.endpoint(serverURL: serverURL) else {
            status = .unknown
            statusFor = nil
            return
        }
        status = .checking
        statusFor = endpoint
        let found = await Self.probe(endpoint)
        // A newer check started meanwhile, or this one was cancelled (the
        // pane went away): either way this answer is not the one to show.
        guard generation == mine, let found else { return }
        status = found
    }

    /// The status for this endpoint, or `.unknown` if the last probe was of a
    /// different one.
    func status(for endpoint: String?) -> AgentDoorStatus {
        guard let endpoint, endpoint == statusFor else { return .unknown }
        return status
    }

    /// One MCP `initialize`, the first thing any agent sends. Deliberately NOT
    /// through APIClient: that would attach the app's bearer token, and a door
    /// that only opens for the token is a door no agent can use.
    /// Nil when the probe was cancelled, which is not an answer about the door.
    nonisolated static func probe(_ endpoint: String) async -> AgentDoorStatus? {
        guard let url = URL(string: endpoint) else { return .unreachable }
        var req = URLRequest(url: url, timeoutInterval: 8)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.setValue("application/json, text/event-stream", forHTTPHeaderField: "Accept")
        req.httpBody = Data(
            #"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"passband-probe","version":"1"}}}"#
                .utf8)
        let session = URLSession(configuration: .ephemeral)
        defer { session.finishTasksAndInvalidate() }
        do {
            let (data, response) = try await session.data(for: req)
            guard let http = response as? HTTPURLResponse else { return .unreachable }
            switch http.statusCode {
            case 200..<300:
                // Leave nothing behind: the daemon keeps a session per
                // initialize until told otherwise.
                if let id = http.value(forHTTPHeaderField: "Mcp-Session-Id") {
                    var close = URLRequest(url: url, timeoutInterval: 4)
                    close.httpMethod = "DELETE"
                    close.setValue(id, forHTTPHeaderField: "Mcp-Session-Id")
                    _ = try? await session.data(for: close)
                }
                // A 200 from something that is not an MCP server (a catch-all
                // web page) is not an open door.
                let body = String(decoding: data, as: UTF8.self)
                return body.contains("\"protocolVersion\"") ? .open : .unreachable
            case 403: return .hostRefused
            case 404, 410: return .notServed
            default: return .unreachable
            }
        } catch {
            if Task.isCancelled || (error as? URLError)?.code == .cancelled { return nil }
            return .unreachable
        }
    }
}
