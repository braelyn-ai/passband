// THE ONE MODAL THAT DOES NOT CLOSE. A dev re-triage rewrites tiers, bands and
// every specialist row underneath whatever page you were reading, and it takes
// minutes — so rather than let the board lie for the duration, this covers it
// and reports the queues draining.
//
// It still has a way out. The re-triage is the DAEMON's work and runs whether
// anyone is watching, so a stuck poll (or a daemon too old to be polled) must
// hand the window back instead of holding it forever behind a counter that
// cannot move. Closing stops the watching, never the run.

import SwiftUI

struct RetriageModal: View {
    @Environment(AppStore.self) private var store
    let run: RetriageRun

    var body: some View {
        // NO DISMISS ON THE SCRIM — deliberately an empty closure rather than an
        // omitted one. The tap still lands here and dies here, which is what
        // stops a click from reaching the board behind it.
        OverlayScrim(onDismiss: {}) {
            ModalCard(width: 380, tint: Palette.glassTintStrong) {
                header
                counter
                bar
                footer
            }
        }
        .keyContext(.modal)
        // Esc is bound to NOTHING while the run is live: the whole point is that
        // the app is unavailable, and a modal that Esc closes is not blocking.
        // Once the wait can no longer end on its own, Esc is the way out.
        .keyBindings(.modal, run.canClose ? [
            KeyBinding("Escape", "close") { store.endRetriage() },
            KeyBinding("Enter", "close") { store.endRetriage() },
        ] : [])
    }

    private var header: some View {
        HStack(spacing: 9) {
            Image(systemName: "arrow.trianglehead.2.clockwise")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Palette.accent)
                .symbolEffect(.rotate, isActive: run.watching && !run.paused)
            Text(title)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(Palette.ink)
            Spacer(minLength: 0)
        }
    }

    private var title: String {
        if run.failure != nil { return "Re-triage: lost track" }
        if run.unsupported { return "Re-triage running" }
        // A CATCH-UP OUTRANKS "not moving", because it is the REASON it is not
        // moving. Triage cannot run until the mailbox finishes its re-walk, so
        // telling somebody the counter is stuck while the daemon is visibly
        // working is the wrong half of the truth.
        if store.catchUp != nil { return "Waiting for your mailbox" }
        // Same reasoning: a spent budget is WHY the counter stopped.
        if run.paused { return "Re-triage paused" }
        if run.stalled { return "Re-triage: not moving" }
        return "Re-triage in progress"
    }

    /// The two numbers, as big as they are because they are the only reason the
    /// window is gone. Before the kick answers there is no denominator yet, and
    /// "0 of 0" is a worse thing to show for that second than a sentence.
    @ViewBuilder private var counter: some View {
        if run.total == 0 && !run.counted {
            HStack {
                Text("sizing the queue…")
                    .font(Typo.num(15))
                    .foregroundStyle(Palette.inkFaint)
                Spacer(minLength: 0)
            }
            .frame(height: 36, alignment: .bottom)
        } else {
            numbers
        }
    }

    private var numbers: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text("\(run.done)")
                .font(Typo.num(30, weight: .semibold))
                .foregroundStyle(Palette.ink)
                .contentTransition(.numericText())
                .animation(.smooth(duration: 0.25), value: run.done)
            Text("of \(run.total)")
                .font(Typo.num(15))
                .foregroundStyle(Palette.inkFaint)
            Text(run.total == 1 ? "email" : "emails")
                .font(Typo.rowSub)
                .foregroundStyle(Palette.inkFaintest)
            Spacer(minLength: 0)
        }
    }

    private var bar: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Palette.hairline)
                Capsule()
                    .fill(run.finished ? Palette.positive : Palette.accent)
                    .frame(width: max(0, geo.size.width * run.fraction))
                    // The counter's own animation, so the number and the bar
                    // arrive together instead of the bar chasing it.
                    .animation(.smooth(duration: 0.25), value: run.fraction)
            }
        }
        .frame(height: 4)
    }

    @ViewBuilder private var footer: some View {
        if let failure = run.failure {
            note(failure, tone: Palette.danger)
            closeButton("Close")
        } else if run.unsupported {
            note(
                "this daemon can't report progress — update squelchd to watch it. "
                    + "The re-triage itself is running.", tone: Palette.warn)
            closeButton("Close")
        } else if let sync = store.catchUp {
            // The numbers are the point: "not moving" and "1,240 of 4,500 and
            // climbing" are the same screen otherwise, and only one of them is
            // worth waiting through.
            note(
                "your mailbox is catching up after being disconnected "
                    + "(\(sync.done) of \(sync.total) messages). Triage starts when it "
                    + "finishes, and this re-triage is queued behind it, not lost.",
                tone: Palette.inkFaintest)
            closeButton("Close")
        } else if run.paused {
            note(
                RetriageRun.pauseNote(resumesAt: Fmt.date(run.resumesAt))
                    + " It carries on by itself; closing this stops the watching, not the run.",
                tone: Palette.warn)
            closeButton("Close")
        } else if run.stalled {
            // Still polling — the run may simply be behind a slow cycle — but
            // the door is open now.
            note(
                "the counter hasn't moved in a while. It may still be working; "
                    + "closing this stops the watching, not the run.", tone: Palette.warn)
            closeButton("Close anyway")
        } else if !run.counted {
            note("asking the daemon where it is…", tone: Palette.inkFaintest)
        } else {
            note(
                "the board is unavailable until this finishes, because every tier "
                    + "on it is about to change.", tone: Palette.inkFaintest)
        }
    }

    private func note(_ text: String, tone: Color) -> some View {
        Text(text)
            .font(Typo.micro)
            .foregroundStyle(tone)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func closeButton(_ label: String) -> some View {
        HStack {
            Spacer(minLength: 0)
            Button(label) { store.endRetriage() }
                .buttonStyle(.glass)
        }
    }
}

// MARK: - the door in front of it

/// THE CONFIRM FOR THE MODAL ABOVE. `re-triage 7d` is a text chip in the
/// masthead, a few points from the sync stamp and the cost line, and until now
/// pressing it took the entire app away for minutes with no way back (#210).
/// One dialog's worth of friction is the whole fix: the run is fine, it just
/// must never start by accident.
///
/// ONE LINE OF SUBTEXT, and it is the wait: the cost of this button is the
/// minutes it takes the window away for, and everything else a longer paragraph
/// could say (the verdicts thrown out, the model spend, what a rule spares) is
/// detail for somebody who already knows what the dev chip does. "At least"
/// rather than a number, because there isn't one: the run is as long as the
/// window's mail, and the daemon does not even say how many rows it reset until
/// after the kick.
struct RetriageConfirm: View {
    @Environment(AppStore.self) private var store
    let days: Int

    var body: some View {
        OverlayScrim(onDismiss: { store.cancelRetriageAsk() }) {
            ModalCard(width: 400) {
                Text("Re-triage the last \(days) days?")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Palette.ink)
                Text("Passband is unavailable until it finishes, at least a few minutes.")
                    .font(Typo.micro)
                    .foregroundStyle(Palette.inkFaint)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                HStack(spacing: 8) {
                    Spacer(minLength: 0)
                    Button("Cancel") { store.cancelRetriageAsk() }
                        .buttonStyle(.glass)
                    Button("Re-triage \(days)d") { Task { await store.confirmRetriage() } }
                        .buttonStyle(.glassProminent)
                        .tint(Palette.warn)
                }
            }
        }
        .keyContext(.modal)
        .keyBindings(.modal, [
            KeyBinding("Escape", "cancel", allowInInput: true) { store.cancelRetriageAsk() },
            KeyBinding("Enter", "re-triage", allowInInput: true) {
                Task { await store.confirmRetriage() }
            },
        ])
    }
}
