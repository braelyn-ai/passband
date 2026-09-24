// ⌘+ / ⌘- / ⌘0: the whole window, scaled the way a browser zooms a page.
//
// WHY THE WHOLE WINDOW AND NOT THE TYPE. Every font in the app is a fixed
// point size (`Typo`'s tokens and a few hundred `.system(size:)` calls), and so
// is every padding, row height and top bar. A text-size setting would have to
// reach all of them and would still leave the spacing at 1x around bigger
// letters. Scaling the shell instead keeps every proportion the design set.
//
// HOW IT REFLOWS rather than just magnifying: the shell is laid out in a box
// `1/zoom` the size of the window and then drawn `zoom` times larger from the
// top-leading corner. At 125% a 1320pt window lays out a 1056pt page, so the
// columns re-fit exactly as they would in a narrower window; nothing is cropped
// and nothing scrolls sideways.

import AppKit
import SwiftUI

enum Zoom {
    /// Safari's ladder, trimmed. Fixed steps rather than a free multiplier so
    /// ⌘0 and a few presses always land back on the same numbers.
    static let steps: [Double] = [0.75, 0.85, 1.0, 1.1, 1.25, 1.5, 1.75]

    /// The layout's own floor (see PassbandApp's `minWidth`/`minHeight`), in
    /// logical points. The window's floor is this times the zoom.
    static let minLogicalSize = CGSize(width: 980, height: 640)

    /// Snap a stored value onto the ladder, so a hand-edited default or a
    /// retired step can never leave the shell at some off-grid scale.
    static func clamp(_ value: Double) -> Double {
        steps.min(by: { abs($0 - value) < abs($1 - value) }) ?? 1
    }

    /// The largest step whose logical floor still fits the screen the window
    /// is on. Past it, the window could not be made big enough to hold the
    /// layout, and AppKit would push it off the display to try.
    @MainActor
    static var ceiling: Double {
        guard let screen = NSApp?.windows.first(where: { $0.isVisible && $0.frame.width > 600 })?
            .screen ?? NSScreen.main
        else { return steps.last ?? 1 }
        let room = screen.visibleFrame.size
        return steps.last(where: {
            minLogicalSize.width * $0 <= room.width && minLogicalSize.height * $0 <= room.height
        }) ?? 1
    }

    @MainActor static func zoomIn() {
        let prefs = Prefs.shared
        guard let next = steps.first(where: { $0 > prefs.zoom + 0.001 }), next <= ceiling else {
            NSSound.beep()
            return
        }
        prefs.zoom = next
    }

    @MainActor static func zoomOut() {
        let prefs = Prefs.shared
        guard let next = steps.last(where: { $0 < prefs.zoom - 0.001 }) else {
            NSSound.beep()
            return
        }
        prefs.zoom = next
    }

    @MainActor static func reset() { Prefs.shared.zoom = 1 }

    @MainActor static var canZoomIn: Bool {
        steps.contains { $0 > Prefs.shared.zoom + 0.001 && $0 <= ceiling }
    }
    @MainActor static var canZoomOut: Bool { Prefs.shared.zoom > (steps.first ?? 1) + 0.001 }
}

/// Lays `content` out at window size / zoom and draws it at zoom.
struct ZoomedWindowContent<Content: View>: View {
    let zoom: Double
    @ViewBuilder let content: Content

    // ONE TREE AT EVERY ZOOM, 1x included. A `zoom == 1` shortcut around the
    // geometry reader was tried and flickered: crossing 100% swapped the
    // branch, which gives the whole shell a new identity, so every piece of
    // view state reset and every email's web view reloaded from scratch.
    var body: some View {
        GeometryReader { geo in
            content
                .frame(width: geo.size.width / zoom, height: geo.size.height / zoom)
                .scaleEffect(zoom, anchor: .topLeading)
        }
        .ignoresSafeArea()
    }
}
