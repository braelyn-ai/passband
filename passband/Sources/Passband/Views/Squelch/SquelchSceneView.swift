// SwiftUI host for the squelch scene. One MTKView, one renderer shared for
// the life of the process (the shader compiles once), and a coordinator that
// owns the scene state and steps it with the display clock.
//
// Reduce Motion gets the settled frame for each beat, drawn on demand: the
// picture still tells the story, it just does not move.

import MetalKit
import QuartzCore
import SwiftUI

struct SquelchSceneView: View {
    /// Whether the squelch is closed: noise stops at the gate, passband leaves.
    let engaged: Bool

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Compiled lazily on first ask. nil means no usable Metal device (or the
    /// shader failed to build), and callers fall back to the static intro.
    @MainActor static let renderer: SquelchRenderer? = SquelchRenderer()

    var body: some View {
        if let renderer = Self.renderer {
            SquelchMetalView(renderer: renderer, engaged: engaged, reduceMotion: reduceMotion)
                .accessibilityElement()
                .accessibilityLabel(engaged
                    ? "Radio noise stops at a glowing gate. Only a narrow band of clear signal passes through."
                    : "A wide field of radio noise pours through a dormant gate.")
                .accessibilityAddTraits(.isImage)
        }
    }
}

@MainActor
final class SquelchCoordinator: NSObject, MTKViewDelegate {
    let renderer: SquelchRenderer
    var engaged = false
    var reduceMotion = false
    private var state = SquelchSceneState()
    private var last: CFTimeInterval?

    init(renderer: SquelchRenderer) {
        self.renderer = renderer
        super.init()
        // Open on a stream that is already in full flow, not one warming up.
        state.time = 8
    }

    /// Jump straight to where the current beat comes to rest.
    func settle() {
        state.engage = engaged ? 1 : 0
        state.camera = engaged ? 1 : 0
        state.filterLo = 0
        state.filterHi = engaged ? SquelchSceneState.xMax + 4 : 0
    }

    nonisolated func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}

    nonisolated func draw(in view: MTKView) {
        MainActor.assumeIsolated { render(in: view) }
    }

    private func render(in view: MTKView) {
        let now = CACurrentMediaTime()
        if !reduceMotion {
            // Clamp so a stall (window hidden, app napped) resumes smoothly
            // rather than fast-forwarding the fronts across the whole stream.
            let dt = Float(min(now - (last ?? now), 1.0 / 20))
            state.step(dt: dt, engaged: engaged)
        }
        last = now
        guard let drawable = view.currentDrawable,
              let buffer = renderer.makeCommandBuffer() else { return }
        renderer.encode(state, into: buffer, target: drawable.texture)
        buffer.present(drawable)
        buffer.commit()
    }
}

@MainActor
private func configure(_ view: MTKView, _ renderer: SquelchRenderer, _ coordinator: SquelchCoordinator) {
    view.device = renderer.device
    view.colorPixelFormat = renderer.colorFormat
    view.depthStencilPixelFormat = .invalid
    view.framebufferOnly = true
    view.preferredFramesPerSecond = 120
    view.delegate = coordinator
}

@MainActor
private func apply(_ view: MTKView, _ coordinator: SquelchCoordinator, engaged: Bool, reduceMotion: Bool) {
    let changed = coordinator.engaged != engaged || coordinator.reduceMotion != reduceMotion
    coordinator.engaged = engaged
    coordinator.reduceMotion = reduceMotion
    view.isPaused = reduceMotion
    view.enableSetNeedsDisplay = reduceMotion
    if reduceMotion && changed {
        coordinator.settle()
        #if os(macOS)
        view.needsDisplay = true
        #else
        view.setNeedsDisplay()
        #endif
    }
}

#if os(macOS)
private struct SquelchMetalView: NSViewRepresentable {
    let renderer: SquelchRenderer
    let engaged: Bool
    let reduceMotion: Bool

    func makeCoordinator() -> SquelchCoordinator { SquelchCoordinator(renderer: renderer) }

    func makeNSView(context: Context) -> MTKView {
        let view = MTKView()
        configure(view, renderer, context.coordinator)
        context.coordinator.engaged = !engaged  // force the first apply to settle
        apply(view, context.coordinator, engaged: engaged, reduceMotion: reduceMotion)
        return view
    }

    func updateNSView(_ view: MTKView, context: Context) {
        apply(view, context.coordinator, engaged: engaged, reduceMotion: reduceMotion)
    }
}
#else
private struct SquelchMetalView: UIViewRepresentable {
    let renderer: SquelchRenderer
    let engaged: Bool
    let reduceMotion: Bool

    func makeCoordinator() -> SquelchCoordinator { SquelchCoordinator(renderer: renderer) }

    func makeUIView(context: Context) -> MTKView {
        let view = MTKView()
        configure(view, renderer, context.coordinator)
        context.coordinator.engaged = !engaged  // force the first apply to settle
        apply(view, context.coordinator, engaged: engaged, reduceMotion: reduceMotion)
        return view
    }

    func updateUIView(_ view: MTKView, context: Context) {
        apply(view, context.coordinator, engaged: engaged, reduceMotion: reduceMotion)
    }
}
#endif
