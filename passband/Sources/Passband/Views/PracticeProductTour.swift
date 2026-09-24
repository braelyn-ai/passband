import SwiftUI

/// A companion to the real UI, never a modal keyboard context. Reader e/t,
/// board navigation, and the actual undo stack continue to work underneath it.
struct PracticeProductTour: View {
    @Environment(AppStore.self) private var store
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.displayScale) private var displayScale
    @State private var frame: CGRect = .zero
    @State private var cardHeight: CGFloat = 270

    private var tour: TourController { store.tour }
    private var step: PracticeTourStep { tour.practiceStep }
    private let cardWidth: CGFloat = 390

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .topLeading) {
                ForEach(Array(ringRects.enumerated()), id: \.offset) { _, ring in
                    RoundedRectangle(cornerRadius: 16)
                        .strokeBorder(Palette.accent, lineWidth: 2)
                        .shadow(color: Palette.accent.opacity(0.25), radius: 14)
                        .frame(width: ring.width, height: ring.height)
                        .offset(x: ring.minX, y: ring.minY)
                        .allowsHitTesting(false)
                }
                if store.ruleEditor == nil {
                    guideCard
                        .id(step)
                        .frame(width: min(cardWidth, max(240, geometry.size.width - 32)))
                        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { cardHeight = $0 }
                        .offset(cardOrigin(in: geometry.size))
                        .transition(reduceMotion ? .opacity : .opacity.combined(with: .offset(y: 8)))
                }
            }
        }
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .named(tourSpace)) } action: { frame = $0 }
        .animation(reduceMotion ? nil : .smooth(duration: 0.2), value: ringRects)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.24), value: step)
        .onChange(of: store.threadId) { _, _ in reconcile() }
        .onChange(of: store.openThreadSummary) { _, _ in reconcile() }
        .onChange(of: store.undos.map(\.id)) { _, _ in reconcile() }
        .onChange(of: store.resolvedIds) { previous, current in
            // Only a new successful restore advances the lesson. Revisiting
            // Undo with Back should show its completed state, not bounce ahead.
            if tour.active, step == .undo, tour.practiceDoneID != nil,
               previous.contains(1), !current.contains(1) {
                tour.advancePractice()
            }
        }
        .onChange(of: step) { _, next in prepare(next) }
        .onAppear { prepare(step) }
        .keyBindings(.global, [
            KeyBinding(declining: "ArrowRight", "next tour step", meta: true) {
                guard store.ruleEditor == nil else { return false }
                next()
                return true
            },
            KeyBinding(declining: "ArrowLeft", "previous tour step", meta: true) {
                guard store.ruleEditor == nil else { return false }
                tour.back()
                return true
            },
            KeyBinding(declining: "Escape", "leave guided tour", meta: true) {
                guard store.ruleEditor == nil else { return false }
                tour.skip()
                return true
            },
        ])
    }

    private var guideCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Label("PASSBAND GUIDE", systemImage: "sparkles")
                    .font(Typo.sectionLabel)
                    .tracking(1)
                    .foregroundStyle(Palette.accentInk)
                Spacer()
                Text("\(step.rawValue + 1) / \(PracticeTourStep.allCases.count)")
                    .font(Typo.num(11))
                    .foregroundStyle(Palette.inkFaint)
            }
            Text(guideTitle)
                .font(Typo.serif(28, weight: .medium))
                .foregroundStyle(Palette.ink)
                .fixedSize(horizontal: false, vertical: true)
            explanationText(guideExplanation)
                .font(.system(size: 15))
                .lineSpacing(3)
                .foregroundStyle(Palette.inkDim)
                .fixedSize(horizontal: false, vertical: true)
            keyboardHint
            if step == .ruleSaved, tour.practiceRuleSaved {
                Label("Your Brightly rule is saved in this practice inbox.", systemImage: "checkmark.circle.fill")
                    .font(Typo.rowSub)
                    .foregroundStyle(Palette.positive)
            }
            primaryAction
            HStack(spacing: 5) {
                ForEach(PracticeTourStep.allCases, id: \.rawValue) { item in
                    Capsule()
                        .fill(item.rawValue <= step.rawValue ? Palette.accent : Palette.ink.opacity(0.1))
                        .frame(height: 3)
                }
            }
            .accessibilityHidden(true)
            HStack(spacing: 14) {
                Button("Back") { tour.back() }
                    .disabled(step == .welcome)
                    .help("Previous step · ⌘←")
                Spacer()
                Button("Skip tour") { tour.skip() }
                    .buttonStyle(.plain)
                    .foregroundStyle(Palette.inkFaintest)
                    .help("Leave the guided tour · ⌘Esc")
                if step != .wrap && step != .welcome {
                    if step.isInteraction && !(step == .undo && undoWasRestored) {
                        Button("Skip step") { next() }
                            .foregroundStyle(Palette.inkFaint)
                            .help("Skip this interaction · ⌘→")
                    } else {
                        Button("Next") { next() }
                            .buttonStyle(.borderedProminent)
                            .tint(Palette.accent)
                            .font(.system(size: 14, weight: .semibold))
                            .controlSize(.large)
                            .help("Next step · ⌘→")
                    }
                }
            }
            .buttonStyle(.textAction)
            .font(Typo.micro)
        }
        .padding(20)
        .background(Color(light: 0xF7FAFD, dark: 0x293747), in: RoundedRectangle(cornerRadius: 20))
        .overlay {
            RoundedRectangle(cornerRadius: 20).strokeBorder(Palette.accent.opacity(0.24))
        }
        .shadow(color: .black.opacity(0.22), radius: 24, y: 10)
    }

    @ViewBuilder
    private var primaryAction: some View {
        if step == .welcome {
            actionButton("Show me around", symbol: "arrow.right") { tour.advancePractice() }
        } else if step == .wrap {
            actionButton("Explore my inbox", symbol: "arrow.right") { tour.completePractice() }
        }
    }

    private func actionButton(_ title: String, symbol: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: symbol)
                .font(.system(size: 13, weight: .semibold))
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.vertical, 4)
        }
        .buttonStyle(.borderedProminent)
        .tint(Palette.accent)
    }

    @ViewBuilder
    private var keyboardHint: some View {
        #if os(macOS)
        switch step {
        case .done:
            guideKeyHint("e", "mark done and return to the board")
        case .undo:
            if !undoWasRestored { guideKeyHint("u", "undo the last action") }
        case .rule:
            guideKeyHint("t", "write a sender rule")
        case .calendar, .shipments:
            if readingRecord { guideKeyHint("esc", "return to your board") }
        default:
            EmptyView()
        }
        #endif
    }

    /// The copy with each `{key}` drawn as the same keycap the hint row uses.
    /// Text cannot host a bordered view, but it can interpolate an Image, so
    /// the cap is rasterized and the sentence still wraps as one paragraph.
    private func explanationText(_ copy: String) -> Text {
        var text = Text(verbatim: "")
        var rest = Substring(copy)
        while let open = rest.firstIndex(of: "{"),
              let close = rest[open...].firstIndex(of: "}") {
            let key = String(rest[rest.index(after: open)..<close])
            text = Text("\(text)\(styledRun(rest[..<open]))\(inlineKeycap(key))")
            rest = rest[rest.index(after: close)...]
        }
        return Text("\(text)\(styledRun(rest))")
    }

    /// `**Section**` runs set in full ink at semibold, so a section name reads
    /// as a place in the app against the dimmed sentence around it.
    private func styledRun(_ run: Substring) -> Text {
        var text = Text(verbatim: "")
        for (index, part) in run.components(separatedBy: "**").enumerated() {
            let piece = Text(verbatim: part)
            text = index.isMultiple(of: 2)
                ? Text("\(text)\(piece)")
                : Text("\(text)\(piece.fontWeight(.semibold).foregroundStyle(Palette.ink))")
        }
        return text
    }

    private func inlineKeycap(_ key: String) -> Text {
        let renderer = ImageRenderer(content: Kbd(key, size: 11)
            .environment(\.colorScheme, colorScheme))
        renderer.scale = displayScale
        #if os(macOS)
        let image = renderer.nsImage.map { Image(nsImage: $0) }
        #else
        let image = renderer.uiImage.map { Image(uiImage: $0) }
        #endif
        guard let image else { return Text(verbatim: key) }
        // Center the cap on the x-height of the 15pt copy around it.
        return Text(image).baselineOffset(-2.5).accessibilityLabel(key == "esc" ? "Escape" : key)
    }

    private func guideKeyHint(_ key: String, _ label: String) -> some View {
        HStack(spacing: 8) {
            Kbd(key).scaleEffect(1.15)
            Text(label).font(.system(size: 13)).foregroundStyle(Palette.inkDim)
        }
    }

    private var readingRecord: Bool {
        tour.practiceRecordThreadID != nil && tour.practiceRecordThreadID == store.threadId
    }

    private var guideTitle: String {
        if step == .ruleSaved {
            if tour.practiceRuleMuted {
                return "Now you’ll never see a Brightly terms-of-service update again."
            }
            return tour.practiceRuleSaved ? "Your rule is saved." : "You’re in control."
        }
        if readingRecord { return "Take a look, then head back." }
        if step == .undo && undoWasRestored { return "Right back where it belongs." }
        return step.title
    }

    private var guideExplanation: String {
        if step == .ruleSaved {
            if tour.practiceRuleMuted {
                return "You can change it at any time. Try “Always show me product updates” or “Only show me discounts 25% off or more.”"
            }
            if !tour.practiceRuleSaved {
                return "Smart rules let you choose how a sender’s mail is handled. You can try one whenever you’re ready."
            }
        }
        if readingRecord {
            return "Here’s the original email. Press {esc} when you’re ready to head back."
        }
        if step == .undo && undoWasRestored {
            return "Maya’s message is back on your board. You’ve tried both finishing a message and bringing it back."
        }
        return step.explanation
    }

    private var ringRects: [CGRect] {
        guard store.threadId == nil, store.activeView == .sitrep else { return [] }
        return step.targets.compactMap { target in
            guard let raw = tour.targets[target] else { return nil }
            let local = raw.offsetBy(dx: -frame.minX, dy: -frame.minY)
            let rect = (step == .openBrightly || step == .openMaya) ? local.insetBy(dx: -4, dy: -4) : local
            let visible = rect.intersection(CGRect(origin: .zero, size: frame.size))
            guard !visible.isNull, visible.width > 50, visible.height > ((step == .openBrightly || step == .openMaya) ? 16 : 35) else { return nil }
            return visible
        }
    }

    private var highlightedBounds: CGRect? {
        ringRects.reduce(nil as CGRect?) { bounds, rect in bounds?.union(rect) ?? rect }
    }

    private func cardOrigin(in size: CGSize) -> CGSize {
        let width = min(cardWidth, max(240, size.width - 32))
        let margin: CGFloat = 18
        let gap: CGFloat = 14
        let maxX = max(margin, size.width - width - margin)
        let maxY = max(margin, size.height - cardHeight - margin)
        guard let target = highlightedBounds else {
            // Leave room for the sidebar and any live undo toast.
            let toastClearance: CGFloat = store.undos.isEmpty ? 0 : 64
            return CGSize(width: min(maxX, max(margin, SidebarRail.railWidth + margin - frame.minX)),
                          height: max(margin, maxY - toastClearance))
        }
        var x: CGFloat
        var y: CGFloat
        // Stay immediately beside the thing being explained. If neither side
        // fits, sit just below or above it instead of drifting to a corner.
        if target.maxX + gap + width <= size.width - margin {
            x = target.maxX + gap
            y = target.midY - cardHeight / 2
        } else if target.minX - gap - width >= margin {
            x = target.minX - gap - width
            y = target.midY - cardHeight / 2
        } else {
            x = target.midX - width / 2
            if target.maxY + gap + cardHeight <= size.height - margin {
                y = target.maxY + gap
            } else if target.minY - gap - cardHeight >= margin {
                y = target.minY - gap - cardHeight
            } else {
                // Small windows cannot fit both surfaces without overlap;
                // retain proximity and keep every guide control reachable.
                x = maxX
                y = target.midY - cardHeight / 2
            }
        }
        return CGSize(width: min(max(x, margin), maxX), height: min(max(y, margin), maxY))
    }

    private func next() {
        if step.isInteraction { tour.skipPracticeStep() }
        else { tour.advancePractice() }
    }

    private func prepare(_ next: PracticeTourStep) {
        guard tour.active else { return }
        if store.activeView != .sitrep { store.setView(.sitrep) }
        // The learner just opened these readers. Preserve their loaded content
        // as the instruction changes instead of closing and fetching it again.
        if next == .done {
            if store.threadId != "practice-1" { openExample(1) }
        } else if next == .rule {
            if store.threadId != "practice-11" { openExample(11) }
        } else { store.closeThread() }
        reconcile()
    }

    private func openExample(_ id: Int) {
        let thread = "practice-\(id)"
        // A real queue is essential: the reader resolves through Actions.done
        // with an undo only when the opened thread has its AttentionUpdate.
        let queue = store.update(id: id).map { [$0] } ?? []
        store.openThread(thread, queue: queue)
    }

    private var undoWasRestored: Bool {
        tour.practiceDoneID != nil && !store.resolvedIds.contains(1)
    }

    private var readerCategory: TourTarget? {
        guard let thread = store.threadId else { return nil }
        if store.zones.calendar.contains(where: { $0.thread_id == thread }) { return .calendar }
        if store.zones.shipments.contains(where: { $0.thread_id == thread }) { return .shipments }
        return nil
    }

    /// Navigation alone is not completion: wait for the matching loaded reader,
    /// successful Done's actual undo entry, and Undo's successful restore.
    private func reconcile() {
        guard tour.active else { return }
        tour.observePracticeRecordReader(
            threadID: store.threadId,
            loadedThreadID: store.currentThreadSummary?.threadId,
            category: readerCategory,
            run: tour.practiceRunID)
        switch step {
        case .openMaya:
            if store.threadId == "practice-1", store.currentThreadSummary?.threadId == "practice-1" {
                tour.advancePractice()
            }
        case .done:
            if let entry = store.undos.last(where: { $0.messageId == 1 && $0.kind == .done }) {
                tour.notePracticeDone(entry.id)
                if store.threadId != "practice-1" { tour.advancePractice() }
            }
        case .openBrightly:
            if store.threadId == "practice-11", store.currentThreadSummary?.threadId == "practice-11" {
                tour.advancePractice()
            }
        default: break
        }
    }
}
