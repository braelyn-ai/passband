// SwiftUI host for the squelch scene.
//
// The scene renders on its own thread, driven by a CAMetalDisplayLink, never
// on the main thread. The intro opens while the rest of the app is still
// starting (and, in rehearsal, while the practice mailbox loads), and a single
// main-thread stall used to freeze the waves for a quarter second right at the
// moment they first appear. Time advances by the display's target presentation
// timestamps, not by when a callback happened to run, so motion stays even
// when a frame is late.
//
// Reduce Motion gets the settled frame for each beat, drawn on demand: the
// picture still tells the story, it just does not move.

import os
import QuartzCore
import SwiftUI

struct SquelchSceneView: View {
    /// Whether the squelch is closed: noise stops at the gate, passband leaves.
    let engaged: Bool

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var colorScheme

    /// Compiled lazily on first ask. nil means no usable Metal device (or the
    /// shader failed to build), and callers fall back to the static intro.
    @MainActor static let renderer: SquelchRenderer? = SquelchRenderer()

    var body: some View {
        if let renderer = Self.renderer {
            SquelchLayerRepresentable(
                renderer: renderer,
                inputs: .init(engaged: engaged, light: colorScheme == .light, reduceMotion: reduceMotion))
                .accessibilityElement()
                .accessibilityLabel(engaged
                    ? "Radio noise stops at a glowing gate. Only a narrow band of clear signal passes through."
                    : "A wide field of radio noise pours through a dormant gate.")
                .accessibilityAddTraits(.isImage)
        }
    }
}

/// What SwiftUI decides; the render thread only ever reads a copy.
struct SquelchInputs: Equatable, Sendable {
    var engaged = false
    var light = false
    var reduceMotion = false
}

/// Owns the render thread and everything it touches. `inputs` is the one
/// shared field, behind a lock; the rest is confined to the render thread.
final class SquelchDriver: NSObject, CAMetalDisplayLinkDelegate, @unchecked Sendable {
    private let renderer: SquelchRenderer
    private let inputs: OSAllocatedUnfairLock<(SquelchInputs, generation: Int)>

    // Render thread only.
    private let targets = SquelchTargets()
    private var state = SquelchSceneState()
    private var started = false
    private var lastPresentation: CFTimeInterval?
    private var drawnGeneration = -1
    private var link: CAMetalDisplayLink?
    // Main thread only.
    private var runLoop: CFRunLoop?

    init(renderer: SquelchRenderer, inputs initial: SquelchInputs) {
        self.renderer = renderer
        self.inputs = OSAllocatedUnfairLock(initialState: (initial, 0))
        super.init()
        // Open on a stream that is already in full flow, not one warming up.
        state.time = 8
    }

    /// Main thread. A change wakes a paused (Reduce Motion) link for one frame.
    func update(_ new: SquelchInputs) {
        let changed = inputs.withLock { current -> Bool in
            guard current.0 != new else { return false }
            current = (new, current.generation + 1)
            return true
        }
        if changed { onRenderThread { $0.link?.isPaused = false } }
    }

    /// Main thread.
    func start(layer: CAMetalLayer) {
        guard runLoop == nil else { return }
        var loop: CFRunLoop?
        let ready = DispatchSemaphore(value: 0)
        let thread = Thread { [self] in
            let link = CAMetalDisplayLink(metalLayer: layer)
            link.delegate = self
            link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
            link.preferredFrameLatency = 2
            link.add(to: .current, forMode: .default)
            self.link = link
            loop = CFRunLoopGetCurrent()
            ready.signal()
            CFRunLoopRun()  // until stop() invalidates the link and stops the loop
        }
        thread.name = "passband.squelch-render"
        thread.qualityOfService = .userInteractive
        thread.start()
        ready.wait()
        runLoop = loop
    }

    /// Main thread.
    func stop() {
        onRenderThread { driver in
            driver.link?.invalidate()
            driver.link = nil
            CFRunLoopStop(CFRunLoopGetCurrent())
        }
        runLoop = nil
    }

    private func onRenderThread(_ body: @escaping @Sendable (SquelchDriver) -> Void) {
        guard let runLoop else { return }
        CFRunLoopPerformBlock(runLoop, CFRunLoopMode.defaultMode.rawValue) { [self] in body(self) }
        CFRunLoopWakeUp(runLoop)
    }

    // Render thread.
    func metalDisplayLink(_ link: CAMetalDisplayLink, needsUpdate update: CAMetalDisplayLink.Update) {
        let (current, generation) = inputs.withLock { $0 }
        let now = update.targetPresentationTimestamp
        if !started {
            // The first frame lands on the beat SwiftUI asked for, never on a
            // default the scene then has to animate away from.
            started = true
            state.settle(engaged: current.engaged, light: current.light)
        }
        if current.reduceMotion {
            if drawnGeneration == generation {
                link.isPaused = true
                return
            }
            state.settle(engaged: current.engaged, light: current.light)
            state.reveal = 1
        } else {
            // Clamp so a stall (window hidden, app napped) resumes smoothly
            // rather than fast-forwarding the fronts across the whole stream.
            let dt = lastPresentation.map { Float(min(now - $0, 1.0 / 20)) } ?? 0
            state.step(dt: dt, engaged: current.engaged, light: current.light)
        }
        lastPresentation = now
        drawnGeneration = generation

        guard let buffer = renderer.makeCommandBuffer() else { return }
        renderer.encode(state, into: buffer, target: update.drawable.texture, targets: targets)
        buffer.present(update.drawable)
        buffer.commit()
    }
}

// MARK: - platform views

#if os(macOS)
final class SquelchLayerView: NSView {
    let driver: SquelchDriver
    private let metalLayer = CAMetalLayer()

    init(renderer: SquelchRenderer, inputs: SquelchInputs) {
        driver = SquelchDriver(renderer: renderer, inputs: inputs)
        super.init(frame: .zero)
        metalLayer.device = renderer.device
        metalLayer.pixelFormat = renderer.colorFormat
        metalLayer.framebufferOnly = true
        metalLayer.isOpaque = true
        metalLayer.maximumDrawableCount = 3
        wantsLayer = true
        layer = metalLayer
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil {
            resize()
            driver.start(layer: metalLayer)
        } else {
            driver.stop()
        }
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        resize()
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        resize()
    }

    private func resize() {
        let scale = window?.backingScaleFactor ?? 2
        metalLayer.contentsScale = scale
        let size = CGSize(width: max(bounds.width * scale, 1), height: max(bounds.height * scale, 1))
        if metalLayer.drawableSize != size { metalLayer.drawableSize = size }
    }
}

private struct SquelchLayerRepresentable: NSViewRepresentable {
    let renderer: SquelchRenderer
    let inputs: SquelchInputs

    func makeNSView(context: Context) -> SquelchLayerView {
        SquelchLayerView(renderer: renderer, inputs: inputs)
    }

    func updateNSView(_ view: SquelchLayerView, context: Context) {
        view.driver.update(inputs)
    }

    static func dismantleNSView(_ view: SquelchLayerView, coordinator: ()) {
        view.driver.stop()
    }
}
#else
final class SquelchLayerView: UIView {
    let driver: SquelchDriver
    override class var layerClass: AnyClass { CAMetalLayer.self }
    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

    init(renderer: SquelchRenderer, inputs: SquelchInputs) {
        driver = SquelchDriver(renderer: renderer, inputs: inputs)
        super.init(frame: .zero)
        metalLayer.device = renderer.device
        metalLayer.pixelFormat = renderer.colorFormat
        metalLayer.framebufferOnly = true
        metalLayer.isOpaque = true
        metalLayer.maximumDrawableCount = 3
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil {
            setNeedsLayout()
            layoutIfNeeded()
            driver.start(layer: metalLayer)
        } else {
            driver.stop()
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let scale = window?.screen.scale ?? traitCollection.displayScale
        metalLayer.contentsScale = scale
        let size = CGSize(width: max(bounds.width * scale, 1), height: max(bounds.height * scale, 1))
        if metalLayer.drawableSize != size { metalLayer.drawableSize = size }
    }
}

private struct SquelchLayerRepresentable: UIViewRepresentable {
    let renderer: SquelchRenderer
    let inputs: SquelchInputs

    func makeUIView(context: Context) -> SquelchLayerView {
        SquelchLayerView(renderer: renderer, inputs: inputs)
    }

    func updateUIView(_ view: SquelchLayerView, context: Context) {
        view.driver.update(inputs)
    }

    static func dismantleUIView(_ view: SquelchLayerView, coordinator: ()) {
        view.driver.stop()
    }
}
#endif

