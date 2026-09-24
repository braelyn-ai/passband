// The onboarding cold open: a spectrum waterfall of broadband noise flowing
// into a squelch gate, with only the passband coming out the other side.
//
// Everything is procedural. The shader ships as source and compiles at first
// use (build.sh is bare swiftc with no Metal toolchain step), every vertex is
// derived from its vertex and instance ids, and there are no buffers, meshes or
// textures. The whole scene costs the bundle its own source text.
//
// Pass order: background, then opaque "curtains" hanging under every band that
// write depth (the Unknown Pleasures hidden-line trick, so near bands occlude
// far ones), then the gate and the glowing lines, both additive and depth
// tested but never depth writing.

import Metal
import simd

struct SquelchSceneState {
    /// Seconds since the scene started. The wave field is a pure function of it.
    var time: Float = 0
    /// 0 = gate dormant, 1 = fully lit. Eased on the CPU.
    var engage: Float = 0
    /// The filtered region downstream of the gate, [lo, hi] in world x. Both
    /// fronts travel at the wave speed, so silence propagates the way a
    /// filtered signal physically would rather than snapping on everywhere.
    var filterLo: Float = 0
    var filterHi: Float = 0
    /// 0 = close in on the noise, 1 = pulled back to see the passband exit.
    var camera: Float = 0
    /// 0 = night (additive glow), 1 = paper (ink). Eased, so a theme change
    /// crossfades instead of cutting.
    var light: Float = 0

    static let waveSpeed: Float = 1.5
    static let xMax: Float = 13

    /// Advance the fronts. `engaged` is the switch; everything else follows.
    mutating func step(dt: Float, engaged: Bool, light isLight: Bool) {
        time += dt
        light += ((isLight ? 1 : 0) - light) * (1 - exp(-dt * 5))
        let c = Self.waveSpeed
        let ease = 1 - exp(-dt * 2.6)
        engage += ((engaged ? 1 : 0) - engage) * ease
        camera += ((engaged ? 1 : 0) - camera) * (1 - exp(-dt * 0.9))
        if engaged {
            if filterLo > 0 { filterLo = 0; filterHi = 0 }
            filterHi = min(filterHi + c * dt, Self.xMax + 4)
        } else if filterHi > 0 {
            // Noise re-enters behind the filtered stretch and chases it out.
            filterLo += c * dt
            filterHi += c * dt
            if filterLo > Self.xMax + 2 { filterLo = 0; filterHi = 0 }
        }
    }
}

final class SquelchRenderer {
    static let bands = 104
    static let segments = 1400
    static let samples = 4

    let device: MTLDevice
    private let queue: MTLCommandQueue
    private let background: MTLRenderPipelineState
    private let curtains: MTLRenderPipelineState
    private let glow: MTLRenderPipelineState
    private let gate: MTLRenderPipelineState
    private let writeDepth: MTLDepthStencilState
    private let testDepth: MTLDepthStencilState
    private let noDepth: MTLDepthStencilState
    private var depthTexture: MTLTexture?
    private var colorTexture: MTLTexture?

    let colorFormat: MTLPixelFormat

    init?(device: MTLDevice? = MTLCreateSystemDefaultDevice(),
          colorFormat: MTLPixelFormat = .bgra8Unorm) {
        guard let device, let queue = device.makeCommandQueue(),
              let library = try? device.makeLibrary(source: squelchShaderSource, options: nil)
        else { return nil }
        self.device = device
        self.queue = queue
        self.colorFormat = colorFormat

        func pipeline(_ vertex: String, _ fragment: String, additive: Bool) -> MTLRenderPipelineState? {
            let d = MTLRenderPipelineDescriptor()
            d.vertexFunction = library.makeFunction(name: vertex)
            d.fragmentFunction = library.makeFunction(name: fragment)
            d.colorAttachments[0].pixelFormat = colorFormat
            d.depthAttachmentPixelFormat = .depth32Float
            d.rasterSampleCount = Self.samples
            if additive {
                let a = d.colorAttachments[0]!
                a.isBlendingEnabled = true
                a.rgbBlendOperation = .add
                a.alphaBlendOperation = .add
                a.sourceRGBBlendFactor = .one
                // Premultiplied over. A fragment with alpha 0 is purely
                // additive, so night glow and paper ink share one state and
                // the theme can crossfade between them.
                a.destinationRGBBlendFactor = .oneMinusSourceAlpha
                a.sourceAlphaBlendFactor = .zero
                a.destinationAlphaBlendFactor = .one
            }
            return try? device.makeRenderPipelineState(descriptor: d)
        }
        func depth(write: Bool, test: Bool) -> MTLDepthStencilState? {
            let d = MTLDepthStencilDescriptor()
            d.depthCompareFunction = test ? .lessEqual : .always
            d.isDepthWriteEnabled = write
            return device.makeDepthStencilState(descriptor: d)
        }
        guard let background = pipeline("bg_vertex", "bg_fragment", additive: false),
              let curtains = pipeline("curtain_vertex", "curtain_fragment", additive: false),
              let glow = pipeline("line_vertex", "line_fragment", additive: true),
              let gate = pipeline("gate_vertex", "gate_fragment", additive: true),
              let writeDepth = depth(write: true, test: true),
              let testDepth = depth(write: false, test: true),
              let noDepth = depth(write: false, test: false)
        else { return nil }
        self.background = background
        self.curtains = curtains
        self.glow = glow
        self.gate = gate
        self.writeDepth = writeDepth
        self.testDepth = testDepth
        self.noDepth = noDepth
    }

    func makeCommandBuffer() -> MTLCommandBuffer? { queue.makeCommandBuffer() }

    /// Encode one frame into `target`. The caller commits (and presents).
    func encode(_ state: SquelchSceneState, into buffer: MTLCommandBuffer, target: MTLTexture) {
        let width = target.width, height = target.height
        if depthTexture?.width != width || depthTexture?.height != height {
            // 4x MSAA: without it the edges where near bands hide far ones
            // crawl as the field scrolls. On Apple GPUs both attachments live
            // in tile memory only, so the samples never touch RAM.
            let memoryless = device.supportsFamily(.apple1)
            func attachment(_ format: MTLPixelFormat) -> MTLTexture? {
                let d = MTLTextureDescriptor.texture2DDescriptor(
                    pixelFormat: format, width: width, height: height, mipmapped: false)
                d.textureType = .type2DMultisample
                d.sampleCount = Self.samples
                d.usage = .renderTarget
                d.storageMode = memoryless ? .memoryless : .private
                return device.makeTexture(descriptor: d)
            }
            depthTexture = attachment(.depth32Float)
            colorTexture = attachment(colorFormat)
        }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = colorTexture
        pass.colorAttachments[0].resolveTexture = target
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .multisampleResolve
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 1)
        pass.depthAttachment.texture = depthTexture
        pass.depthAttachment.loadAction = .clear
        pass.depthAttachment.storeAction = .dontCare
        pass.depthAttachment.clearDepth = 1
        guard let enc = buffer.makeRenderCommandEncoder(descriptor: pass) else { return }

        var u = uniforms(state, width: Float(width), height: Float(height))
        let size = MemoryLayout<SquelchUniforms>.stride
        let stripVertices = (Self.segments + 1) * 2

        enc.setDepthStencilState(noDepth)
        enc.setRenderPipelineState(background)
        enc.setVertexBytes(&u, length: size, index: 0)
        enc.setFragmentBytes(&u, length: size, index: 0)
        enc.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)

        enc.setDepthStencilState(writeDepth)
        enc.setRenderPipelineState(curtains)
        enc.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: stripVertices,
                           instanceCount: Self.bands)

        enc.setDepthStencilState(testDepth)
        enc.setRenderPipelineState(gate)
        enc.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)

        enc.setRenderPipelineState(glow)
        enc.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: stripVertices,
                           instanceCount: Self.bands)
        enc.endEncoding()
    }

    private func uniforms(_ s: SquelchSceneState, width: Float, height: Float) -> SquelchUniforms {
        let aspect = width / max(height, 1)
        // Close and low in the noise, pulling up and back to frame the exit.
        let k = s.camera * s.camera * (3 - 2 * s.camera)
        let drift = SIMD3<Float>(sin(s.time * 0.07) * 0.6, sin(s.time * 0.05) * 0.25, 0)
        let eye = mix(SIMD3<Float>(4.2, 2.6, 8.4), SIMD3<Float>(9.4, 5.0, 12.4), t: k) + drift
        let target = mix(SIMD3<Float>(-1.6, 0.1, 0), SIMD3<Float>(2.6, -0.1, 0.6), t: k)
        // Wide windows get the scene pushed right so the copy has room.
        // Tall screens widen the lens to keep roughly the same horizontal field.
        let fovY = aspect < 1.2 ? 2 * atan(tan(0.31) * 0.78 / aspect) : 0.62
        let lens = matrix_float4x4.perspective(fovY: fovY, aspect: aspect, near: 0.1, far: 80)
        let shift = matrix_float4x4.translation(SIMD3<Float>(aspect > 1.3 ? 0.28 : 0, aspect > 1.3 ? 0 : -0.1, 0))
        let view = matrix_float4x4.lookAt(eye: eye, target: target, up: SIMD3<Float>(0, 1, 0))
        return SquelchUniforms(
            viewProj: shift * lens * view,
            eye: SIMD4<Float>(eye, s.time),
            frame: SIMD4<Float>(width, height, s.filterLo, s.filterHi),
            field: SIMD4<Float>(Float(Self.bands), Float(Self.segments), -17, SquelchSceneState.xMax),
            filter: SIMD4<Float>(s.engage, SquelchSceneState.waveSpeed, s.light, 0))
    }
}

/// Mirrors `struct U` in the shader. SIMD4 everywhere keeps the layouts equal.
private struct SquelchUniforms {
    var viewProj: matrix_float4x4
    var eye: SIMD4<Float>     // xyz camera, w time
    var frame: SIMD4<Float>   // viewport px w/h, filter lo/hi
    var field: SIMD4<Float>   // bands, segments, x min, x max
    var filter: SIMD4<Float>  // engage, wave speed, light
}

private func mix(_ a: SIMD3<Float>, _ b: SIMD3<Float>, t: Float) -> SIMD3<Float> { a + (b - a) * t }

private extension matrix_float4x4 {
    static func perspective(fovY: Float, aspect: Float, near: Float, far: Float) -> matrix_float4x4 {
        let y = 1 / tan(fovY / 2), x = y / aspect, z = far / (near - far)
        return matrix_float4x4(columns: (
            SIMD4<Float>(x, 0, 0, 0), SIMD4<Float>(0, y, 0, 0),
            SIMD4<Float>(0, 0, z, -1), SIMD4<Float>(0, 0, z * near, 0)))
    }

    static func lookAt(eye: SIMD3<Float>, target: SIMD3<Float>, up: SIMD3<Float>) -> matrix_float4x4 {
        let f = simd_normalize(target - eye)
        let s = simd_normalize(simd_cross(f, up))
        let u = simd_cross(s, f)
        return matrix_float4x4(columns: (
            SIMD4<Float>(s.x, u.x, -f.x, 0), SIMD4<Float>(s.y, u.y, -f.y, 0),
            SIMD4<Float>(s.z, u.z, -f.z, 0),
            SIMD4<Float>(-simd_dot(s, eye), -simd_dot(u, eye), simd_dot(f, eye), 1)))
    }

    static func translation(_ t: SIMD3<Float>) -> matrix_float4x4 {
        var m = matrix_identity_float4x4
        m.columns.3 = SIMD4<Float>(t, 1)
        return m
    }
}

// MARK: - shader

private let squelchShaderSource = #"""
#include <metal_stdlib>
using namespace metal;

struct U {
    float4x4 viewProj;
    float4 eye;
    float4 frame;
    float4 field;
    float4 filter;
};

// World layout: waves travel +x, bands are stacked along z, the gate is x = 0.
constant float BAND_SPAN = 11.0;

float h11(float p) { p = fract(p * 0.1031); p *= p + 33.33; p *= p + p; return fract(p); }
float h21(float2 p) {
    float3 q = fract(float3(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}
float vnoise(float2 p) {
    float2 i = floor(p), f = fract(p);
    float2 w = f * f * (3.0 - 2.0 * f);
    float a = h21(i), b = h21(i + float2(1, 0)), c = h21(i + float2(0, 1)), d = h21(i + float2(1, 1));
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y) * 2.0 - 1.0;
}
float fbm(float2 p) {
    float s = 0.0, a = 0.55;
    // Four octaves, the last at half weight: the finest detail has to stay
    // under what one line segment can sample, or it crawls as it scrolls.
    for (int k = 0; k < 4; k++) {
        s += a * vnoise(p) * (k == 3 ? 0.5 : 1.0);
        p = p * 2.07 + float2(17.1, 3.7);
        a *= 0.52;
    }
    return s * 1.12;
}

float bandZ(float i, constant U& u) { return (i / (u.field.x - 1.0) - 0.5) * BAND_SPAN; }

// Where the passband is tuned at time tau, in [-1, 1]. Hold, then glide to a
// new target, on a warped clock so the pace never settles into a rhythm: it
// should read as a decision being made, not a metronome.
float passOffset(float tau) {
    float w = tau * 0.30 + 0.45 * sin(tau * 0.23);   // monotonic: 0.30 > 0.45 * 0.23
    float k = floor(w), f = fract(w);
    float a = h11(k * 7.31 + 1.7) * 2.0 - 1.0;
    float b = h11((k + 1.0) * 7.31 + 1.7) * 2.0 - 1.0;
    float e = saturate((f - 0.5) / 0.5);
    e = e * e * e * (e * (e * 6.0 - 15.0) + 10.0);
    return mix(a, b, e) + 0.06 * sin(tau * 1.7) * sin(tau * 0.61);
}

// The passband's magnitude response: an 8th-order Butterworth over band index,
// centered wherever it was tuned when this stretch of wave crossed the gate.
// Evaluating the tuning at the retarded time -s/c is what makes the filtered
// stream downstream meander: it is the tuning's own history, carried out.
float gain(float i, float s, constant U& u) {
    float tau = -s / u.filter.y;
    float c = (u.field.x - 1.0) * (0.5 + 0.17 * passOffset(tau));
    float r = (i - c) / 4.2;
    return 1.0 / sqrt(1.0 + pow(r * r, 8.0));
}

// Broadband noise, a function of the retarded coordinate s = x - c t so it
// travels as one field. Bursts ride along with it.
float noiseAmp(float i, float s, constant U& u) {
    float n = i / (u.field.x - 1.0);
    float hump = exp(-pow((n - 0.5) * 2.6, 2.0));
    float burst = 0.3 + 0.9 * smoothstep(-0.15, 0.7, vnoise(float2(s * 0.16, i * 0.31)));
    return (0.16 + 0.95 * hump) * burst;
}
// The noise itself. Each octave's temporal frequency (spatial frequency times
// how fast it slides) is capped well under the display rate: when every octave
// advected at the wave speed, the finest detail slid ~its own width per frame
// and strobed, and anything
// much above ~3 Hz across a hundred dense lines is tiring to look at. The broad shapes still flow downstream at full speed; the fine
// grain drifts slower and churns in place, which reads as noise, not as lag.
float noiseY(float i, float x, float t, constant U& u) {
    float c = u.filter.y;
    float s = x - c * t;
    float sum = 0.0, a = 0.55, f = 2.7;
    float2 off = float2(0.0, i * 1.93);
    for (int k = 0; k < 4; k++) {
        float v = min(c, 3.0 / f);             // cap: f * v <= 3 cycles/s
        float2 p = float2(x * f - t * f * v, off.y) + float2(off.x, 0.0);
        sum += a * vnoise(p) * (k == 3 ? 0.5 : 1.0);
        off = off * 2.07 + float2(17.1, 3.7);
        a *= 0.52;
        f *= 2.07;
    }
    return noiseAmp(i, s, u) * sum * 1.12;
}

// The signal: wave packets on a carrier, one per stretch of stream per band.
float packetEnv(float i, float s) {
    float P = 6.5;
    float n = floor(s / P);
    float r = h11(n * 13.7 + i * 0.71);
    float center = (n + 0.25 + 0.5 * r) * P;
    return exp(-pow((s - center) / 0.85, 2.0)) * (0.65 + 0.55 * h11(n * 3.1 + 2.0));
}
float signalY(float i, float s) {
    float e = packetEnv(i, s);
    return e * (0.42 + 0.58 * sin(s * 13.0)) * 0.75;
}

// How filtered this point is: 0 upstream / in untouched noise, 1 inside the
// squelched stretch. The gate itself is a short ramp, not a cliff.
float filtered(float x, constant U& u) {
    float lo = u.frame.z, hi = u.frame.w;
    return saturate((hi - x) / 0.9) * saturate((x - lo) / 0.9) * smoothstep(-0.2, 0.35, x);
}

// glow: night color, added. ink + alpha: paper color, composited over.
struct Sample { float3 p; float3 glow; float3 ink; float alpha; };

Sample field(float i, float x, constant U& u) {
    float t = u.eye.w;
    float s = x - u.filter.y * t;
    float g = gain(i, s, u);
    float m = filtered(x, u);
    float noise = noiseY(i, x, t, u);
    float sig = signalY(i, s);
    float raw = noise + g * sig * 0.8;
    float clean = g * sig * 1.35;
    float y = mix(raw, clean, m);

    float n = i / (u.field.x - 1.0);
    float env = packetEnv(i, s) * g;
    // Brightness follows the burst envelope, never the instantaneous sample:
    // tying it to |noise| made every line twinkle along its length.
    float busy = saturate(noiseAmp(i, s, u) * 0.75);

    // Night: a muted spectrum, warm lows to cool highs; the passband in
    // passband blue with the packets running hot.
    float3 noiseG = mix(float3(0.55, 0.34, 0.62), float3(0.22, 0.52, 0.66), n) * (0.42 + 0.4 * busy);
    float3 passG = float3(0.30, 0.62, 1.00) * (0.9 + 0.5 * g) + float3(1.0, 0.86, 0.62) * env * 2.2;
    float3 glow = mix(noiseG, mix(float3(0.06, 0.085, 0.12), passG, g), m);

    // Paper: the same spectrum as ink, the passband in accent blue deepening
    // to navy through each packet.
    // Slate, not black: dense high-contrast stripes make a page vibrate.
    float3 noiseI = mix(float3(0.42, 0.38, 0.58), float3(0.30, 0.46, 0.56), n);
    float noiseA = 0.26 + 0.26 * busy;
    float3 passI = mix(float3(0.17, 0.50, 0.83), float3(0.11, 0.39, 0.68), saturate(env * 1.4));
    float3 ink = mix(noiseI, mix(float3(0.55, 0.62, 0.72), passI, g), m);
    float alpha = mix(noiseA, mix(0.07, 0.78, g), m);

    // Energy dumped at the gate: out-of-band lines flare where they die.
    float flare = exp(-x * x / 0.06) * u.filter.x * (1.0 - g) * (0.5 + busy);
    glow += float3(0.55, 0.75, 1.0) * flare;
    ink = mix(ink, float3(0.17, 0.50, 0.83), saturate(flare));
    alpha = saturate(alpha + flare * 0.5);

    // Fade the stream in and out at its ends, and the outermost bands.
    float fade = smoothstep(u.field.z, u.field.z + 6.0, x) * smoothstep(u.field.w, u.field.w - 4.0, x);
    fade *= smoothstep(0.0, 0.08, n) * smoothstep(1.0, 0.92, n);
    return { float3(x, y, bandZ(i, u)), glow * fade, ink, alpha * fade };
}

// Night and paper, t = 0..1 between them.
float3 backdrop(float2 uv, float t) {
    // uv 0..1, y down. A faint bloom toward the gate side.
    float r = length((uv - float2(0.62, 0.45)) * float2(1.0, 1.4));
    float bloom = exp(-r * r * 3.0);
    float3 night = mix(float3(0.035, 0.05, 0.085), float3(0.012, 0.016, 0.03), uv.y)
        + float3(0.05, 0.09, 0.16) * bloom;
    // The app's canvas (EDF3FA) rather than white: less glare behind ink.
    float3 paper = mix(float3(0.93, 0.953, 0.98), float3(0.895, 0.918, 0.95), uv.y)
        - float3(0.03, 0.018, 0.0) * bloom;
    return mix(night, paper, t);
}

// MARK: background

struct BgOut { float4 pos [[position]]; float2 uv; };

vertex BgOut bg_vertex(uint vid [[vertex_id]]) {
    float2 p = float2((vid << 1) & 2, vid & 2);
    BgOut o;
    o.pos = float4(p * 2.0 - 1.0, 1.0, 1.0);
    o.uv = float2(p.x, 1.0 - p.y);
    return o;
}

fragment float4 bg_fragment(BgOut in [[stage_in]], constant U& u [[buffer(0)]]) {
    return float4(backdrop(in.uv, u.filter.z), 1.0);
}

// MARK: curtains (occluders)

struct CurtainOut { float4 pos [[position]]; };

vertex CurtainOut curtain_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                                 constant U& u [[buffer(0)]]) {
    float i = float(iid);
    float j = float(vid >> 1);
    float x = mix(u.field.z, u.field.w, j / u.field.y);
    Sample s = field(i, x, u);
    float3 p = s.p;
    if (vid & 1) p.y = -4.0; else p.y -= 0.035;
    CurtainOut o;
    o.pos = u.viewProj * float4(p, 1.0);
    return o;
}

fragment float4 curtain_fragment(CurtainOut in [[stage_in]], constant U& u [[buffer(0)]]) {
    return float4(backdrop(in.pos.xy / u.frame.xy, u.filter.z), 1.0);
}

// MARK: lines

struct LineOut { float4 pos [[position]]; float3 glow; float3 ink; float alpha; float across; };

vertex LineOut line_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                           constant U& u [[buffer(0)]]) {
    float i = float(iid);
    float j = float(vid >> 1);
    float side = (vid & 1) ? 1.0 : -1.0;
    float dx = (u.field.w - u.field.z) / u.field.y;
    float x = u.field.z + j * dx;
    Sample a = field(i, x, u);
    Sample b = field(i, x + dx * 0.5, u);
    float4 ca = u.viewProj * float4(a.p, 1.0);
    float4 cb = u.viewProj * float4(b.p, 1.0);
    float2 sa = ca.xy / ca.w * u.frame.xy, sb = cb.xy / cb.w * u.frame.xy;
    float2 d = normalize(sb - sa + 1e-5);
    float2 nrm = float2(-d.y, d.x);
    // Width in pixels, a touch thicker up close. The glow tail needs room.
    float px = clamp(34.0 / ca.w, 2.2, 7.0) * (u.frame.y / 900.0);
    LineOut o;
    o.pos = ca;
    o.pos.xy += nrm * side * px * 2.0 / u.frame.xy * ca.w;
    // Far bands crowd to under a pixel apart and shimmer; let them recede.
    float recede = mix(1.0, 0.5, smoothstep(12.0, 24.0, ca.w));
    o.glow = a.glow * recede;
    o.ink = a.ink;
    o.alpha = a.alpha * recede;
    o.across = side;
    return o;
}

fragment float4 line_fragment(LineOut in [[stage_in]], constant U& u [[buffer(0)]]) {
    float d = abs(in.across);
    float core = exp(-d * d * 18.0);
    float halo = exp(-d * d * 3.0) * 0.35;
    float t = u.filter.z;
    float3 night = in.glow * (core * 1.3 + halo);
    // Ink has no glow to spend, so it gets a crisper core and a thin halo.
    float a = saturate(in.alpha * (core * 1.2 + halo * 0.5));
    return float4(mix(night, in.ink * a, t), a * t);
}

// MARK: gate

struct GateOut { float4 pos [[position]]; float2 q; };

vertex GateOut gate_vertex(uint vid [[vertex_id]], constant U& u [[buffer(0)]]) {
    float2 q = float2(vid & 1, vid >> 1);        // 0..1 over the pane
    float z = (q.x - 0.5) * (BAND_SPAN + 1.6);
    float y = mix(-0.12, 2.0, q.y);
    GateOut o;
    o.pos = u.viewProj * float4(0.0, y, z, 1.0);
    o.q = q;
    return o;
}

fragment float4 gate_fragment(GateOut in [[stage_in]], constant U& u [[buffer(0)]]) {
    float e = u.filter.x;
    float2 q = in.q;
    float2 size = float2(BAND_SPAN + 1.6, 2.5);
    float2 edge = min(q, 1.0 - q) * size;
    // A hairline frame, brightest along the base where the waves cross.
    float frame = exp(-min(edge.x, edge.y) * 38.0) * (0.55 + 0.45 * (1.0 - q.y));
    // The aperture: the passband's own response curve, drawn as a lit slot.
    float z = (q.x - 0.5) * size.x;
    float i = (z / BAND_SPAN + 0.5) * (u.field.x - 1.0);
    float g = gain(i, -u.filter.y * u.eye.w, u);
    float slotEdge = exp(-pow((g - 0.5) * 7.0, 2.0));
    float slot = g * 0.10 * (1.0 - q.y) + slotEdge * 0.55;
    float glass = 0.024 * (1.0 - q.y * 0.7);
    float lit = 0.1 + 0.9 * e;
    float3 night = (float3(0.35, 0.62, 1.0) * (frame + glass) + float3(0.62, 0.82, 1.0) * slot) * lit;
    float a = saturate((frame * 0.9 + glass * 2.5 + slot * 0.8) * lit);
    float t = u.filter.z;
    return float4(mix(night, float3(0.17, 0.50, 0.83) * a, t), a * t);
}
"""#
