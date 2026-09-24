// The onboarding cold open, ported from the Swift client's Metal scene
// (passband/Sources/Passband/Views/Squelch/SquelchRenderer.swift) to WebGL2.
//
// Same scene, same numbers: a spectrum waterfall of broadband noise flowing
// into a squelch gate, with only the passband coming out the other side. The
// site is always night, so the paper theme's ink path is dropped and every
// blend is purely additive.
//
// Everything is procedural. No buffers, meshes or textures: every vertex is
// derived from gl_VertexID and gl_InstanceID, exactly as the Metal version
// derives it from its vertex and instance ids.
//
// Pass order: background, then opaque "curtains" hanging under every band that
// write depth (the Unknown Pleasures hidden-line trick, so near bands occlude
// far ones), then the gate and the glowing lines, both additive and depth
// tested but never depth writing.

const BANDS = 104;
const WAVE_SPEED = 2.4;
const X_MIN = -17;
const X_MAX = 13;

/// Mirrors SquelchSceneState. The wave field is a pure function of `time`.
class SceneState {
  time = 0;
  engage = 0;
  filterLo = 0;
  filterHi = 0;
  camera = 0;

  /// Advance the fronts. `engaged` is the switch; the camera follows its own
  /// target so the page's scroll can drive the pull-back directly.
  step(dt: number, engaged: boolean, cameraTarget: number) {
    this.time += dt;
    const c = WAVE_SPEED;
    this.engage += ((engaged ? 1 : 0) - this.engage) * (1 - Math.exp(-dt * 2.6));
    this.camera += (cameraTarget - this.camera) * (1 - Math.exp(-dt * 2.2));
    if (engaged) {
      if (this.filterLo > 0) {
        this.filterLo = 0;
        this.filterHi = 0;
      }
      this.filterHi = Math.min(this.filterHi + c * dt, X_MAX + 4);
    } else if (this.filterHi > 0) {
      // Noise re-enters behind the filtered stretch and chases it out.
      this.filterLo += c * dt;
      this.filterHi += c * dt;
      if (this.filterLo > X_MAX + 2) {
        this.filterLo = 0;
        this.filterHi = 0;
      }
    }
  }
}

// MARK: - matrices (column-major, matching simd's float4x4 columns)

type Vec3 = [number, number, number];
type Mat4 = Float32Array;

function perspective(fovY: number, aspect: number, near: number, far: number): Mat4 {
  // Metal's 0..1 depth range. In WebGL that lands inside -1..1, which is all
  // the depth test needs; the near half of the range simply goes unused.
  const y = 1 / Math.tan(fovY / 2);
  const x = y / aspect;
  const z = far / (near - far);
  return new Float32Array([x, 0, 0, 0, 0, y, 0, 0, 0, 0, z, -1, 0, 0, z * near, 0]);
}

function lookAt(eye: Vec3, target: Vec3): Mat4 {
  const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
  const norm = (a: Vec3): Vec3 => {
    const l = Math.hypot(a[0], a[1], a[2]) || 1;
    return [a[0] / l, a[1] / l, a[2] / l];
  };
  const cross = (a: Vec3, b: Vec3): Vec3 => [
    a[1] * b[2] - a[2] * b[1],
    a[2] * b[0] - a[0] * b[2],
    a[0] * b[1] - a[1] * b[0],
  ];
  const dot = (a: Vec3, b: Vec3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  const f = norm(sub(target, eye));
  const s = norm(cross(f, [0, 1, 0]));
  const u = cross(s, f);
  return new Float32Array([
    s[0], u[0], -f[0], 0,
    s[1], u[1], -f[1], 0,
    s[2], u[2], -f[2], 0,
    -dot(s, eye), -dot(u, eye), dot(f, eye), 1,
  ]);
}

function multiply(a: Mat4, b: Mat4): Mat4 {
  const out = new Float32Array(16);
  for (let c = 0; c < 4; c++)
    for (let r = 0; r < 4; r++) {
      let v = 0;
      for (let k = 0; k < 4; k++) v += a[k * 4 + r] * b[c * 4 + k];
      out[c * 4 + r] = v;
    }
  return out;
}

function translation(x: number, y: number): Mat4 {
  return new Float32Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, x, y, 0, 1]);
}

// MARK: - shader

const COMMON = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

uniform mat4 uViewProj;
uniform vec4 uEye;     // xyz camera, w time
uniform vec4 uFrame;   // viewport px w/h, filter lo/hi
uniform vec4 uField;   // bands, segments, x min, x max
uniform vec4 uFilter;  // engage, wave speed

// World layout: waves travel +x, bands are stacked along z, the gate is x = 0.
const float BAND_SPAN = 11.0;

float sat(float x) { return clamp(x, 0.0, 1.0); }
float sq(float x) { return x * x; }

float h11(float p) { p = fract(p * 0.1031); p *= p + 33.33; p *= p + p; return fract(p); }
float h21(vec2 p) {
    vec3 q = fract(vec3(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}
float vnoise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    vec2 w = f * f * (3.0 - 2.0 * f);
    float a = h21(i), b = h21(i + vec2(1, 0)), c = h21(i + vec2(0, 1)), d = h21(i + vec2(1, 1));
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y) * 2.0 - 1.0;
}
float fbm(vec2 p) {
    float s = 0.0, a = 0.55;
    // Four octaves, the last at half weight: the finest detail has to stay
    // under what one line segment can sample, or it crawls as it scrolls.
    for (int k = 0; k < 4; k++) {
        s += a * vnoise(p) * (k == 3 ? 0.5 : 1.0);
        p = p * 2.07 + vec2(17.1, 3.7);
        a *= 0.52;
    }
    return s * 1.12;
}

float bandZ(float i) { return (i / (uField.x - 1.0) - 0.5) * BAND_SPAN; }

// Where the passband is tuned at time tau, in [-1, 1]. Hold, then glide to a
// new target, on a warped clock so the pace never settles into a rhythm.
float passOffset(float tau) {
    float w = tau * 0.30 + 0.45 * sin(tau * 0.23);
    float k = floor(w), f = fract(w);
    float a = h11(k * 7.31 + 1.7) * 2.0 - 1.0;
    float b = h11((k + 1.0) * 7.31 + 1.7) * 2.0 - 1.0;
    float e = sat((f - 0.5) / 0.5);
    e = e * e * e * (e * (e * 6.0 - 15.0) + 10.0);
    return mix(a, b, e) + 0.06 * sin(tau * 1.7) * sin(tau * 0.61);
}

// The passband's magnitude response: an 8th-order Butterworth over band index,
// centered wherever it was tuned when this stretch of wave crossed the gate.
float gain(float i, float s) {
    float tau = -s / uFilter.y;
    float c = (uField.x - 1.0) * (0.5 + 0.17 * passOffset(tau));
    float r = (i - c) / 4.2;
    return 1.0 / sqrt(1.0 + pow(r * r, 8.0));
}

// Broadband noise, a function of the retarded coordinate s = x - c t so it
// travels as one field. Bursts ride along with it.
// (GLSL's pow is undefined for a negative base, so squares are spelled out.)
float noiseAmp(float i, float s) {
    float n = i / (uField.x - 1.0);
    float hump = exp(-sq((n - 0.5) * 2.6));
    float burst = 0.3 + 0.9 * smoothstep(-0.15, 0.7, vnoise(vec2(s * 0.16, i * 0.31)));
    return (0.16 + 0.95 * hump) * burst;
}
float noiseY(float i, float x, float t) {
    float s = x - uFilter.y * t;
    return noiseAmp(i, s) * fbm(vec2(s * 2.7, i * 1.93));
}

// The signal: wave packets on a carrier, one per stretch of stream per band.
float packetEnv(float i, float s) {
    float P = 6.5;
    float n = floor(s / P);
    float r = h11(n * 13.7 + i * 0.71);
    float center = (n + 0.25 + 0.5 * r) * P;
    return exp(-sq((s - center) / 0.85)) * (0.65 + 0.55 * h11(n * 3.1 + 2.0));
}
float signalY(float i, float s) {
    float e = packetEnv(i, s);
    return e * (0.42 + 0.58 * sin(s * 13.0)) * 0.75;
}

// How filtered this point is: 0 upstream / in untouched noise, 1 inside the
// squelched stretch. The gate itself is a short ramp, not a cliff.
float filtered(float x) {
    float lo = uFrame.z, hi = uFrame.w;
    return sat((hi - x) / 0.9) * sat((x - lo) / 0.9) * smoothstep(-0.2, 0.35, x);
}

struct Sample { vec3 p; vec3 glow; };

Sample field(float i, float x) {
    float t = uEye.w;
    float s = x - uFilter.y * t;
    float g = gain(i, s);
    float m = filtered(x);
    float noise = noiseY(i, x, t);
    float sig = signalY(i, s);
    float raw = noise + g * sig * 0.8;
    float clean = g * sig * 1.35;
    float y = mix(raw, clean, m);

    float n = i / (uField.x - 1.0);
    float env = packetEnv(i, s) * g;
    // Brightness follows the burst envelope, never the instantaneous sample.
    float busy = sat(noiseAmp(i, s) * 0.75);

    // A muted spectrum, warm lows to cool highs; the passband in passband
    // blue with the packets running hot.
    vec3 noiseG = mix(vec3(0.55, 0.34, 0.62), vec3(0.22, 0.52, 0.66), n) * (0.42 + 0.4 * busy);
    vec3 passG = vec3(0.30, 0.62, 1.00) * (0.9 + 0.5 * g) + vec3(1.0, 0.86, 0.62) * env * 2.2;
    vec3 glow = mix(noiseG, mix(vec3(0.06, 0.085, 0.12), passG, g), m);

    // Energy dumped at the gate: out-of-band lines flare where they die.
    float flare = exp(-x * x / 0.06) * uFilter.x * (1.0 - g) * (0.5 + busy);
    glow += vec3(0.55, 0.75, 1.0) * flare;

    // Fade the stream in and out at its ends, and the outermost bands.
    float fade = smoothstep(uField.z, uField.z + 6.0, x) * smoothstep(uField.w, uField.w - 4.0, x);
    fade *= smoothstep(0.0, 0.08, n) * smoothstep(1.0, 0.92, n);
    return Sample(vec3(x, y, bandZ(i)), glow * fade);
}

// uv 0..1, y down. A faint bloom toward the gate side.
vec3 backdrop(vec2 uv) {
    float r = length((uv - vec2(0.62, 0.45)) * vec2(1.0, 1.4));
    float bloom = exp(-r * r * 3.0);
    return mix(vec3(0.035, 0.05, 0.085), vec3(0.012, 0.016, 0.03), uv.y)
        + vec3(0.05, 0.09, 0.16) * bloom;
}
`;

const BG_VS = COMMON + /* glsl */ `
out vec2 vUv;
void main() {
    vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    gl_Position = vec4(p * 2.0 - 1.0, 1.0, 1.0);
    vUv = vec2(p.x, 1.0 - p.y);
}`;
const BG_FS = COMMON + /* glsl */ `
in vec2 vUv;
out vec4 color;
void main() { color = vec4(backdrop(vUv), 1.0); }`;

const CURTAIN_VS = COMMON + /* glsl */ `
void main() {
    float i = float(gl_InstanceID);
    float j = float(gl_VertexID >> 1);
    float x = mix(uField.z, uField.w, j / uField.y);
    vec3 p = field(i, x).p;
    if ((gl_VertexID & 1) == 1) p.y = -4.0; else p.y -= 0.035;
    gl_Position = uViewProj * vec4(p, 1.0);
}`;
const CURTAIN_FS = COMMON + /* glsl */ `
out vec4 color;
void main() {
    // gl_FragCoord is y up; backdrop() wants Metal's y down.
    vec2 uv = vec2(gl_FragCoord.x, uFrame.y - gl_FragCoord.y) / uFrame.xy;
    color = vec4(backdrop(uv), 1.0);
}`;

const LINE_VS = COMMON + /* glsl */ `
out vec3 vGlow;
out float vAcross;
void main() {
    float i = float(gl_InstanceID);
    float j = float(gl_VertexID >> 1);
    float side = (gl_VertexID & 1) == 1 ? 1.0 : -1.0;
    float dx = (uField.w - uField.z) / uField.y;
    float x = uField.z + j * dx;
    Sample a = field(i, x);
    Sample b = field(i, x + dx * 0.5);
    vec4 ca = uViewProj * vec4(a.p, 1.0);
    vec4 cb = uViewProj * vec4(b.p, 1.0);
    vec2 sa = ca.xy / ca.w * uFrame.xy, sb = cb.xy / cb.w * uFrame.xy;
    vec2 d = normalize(sb - sa + 1e-5);
    vec2 nrm = vec2(-d.y, d.x);
    // Width in pixels, a touch thicker up close. The glow tail needs room.
    float px = clamp(34.0 / ca.w, 2.2, 7.0) * (uFrame.y / 900.0);
    gl_Position = ca;
    gl_Position.xy += nrm * side * px * 2.0 / uFrame.xy * ca.w;
    // Far bands crowd to under a pixel apart and shimmer; let them recede.
    float recede = mix(1.0, 0.5, smoothstep(12.0, 24.0, ca.w));
    vGlow = a.glow * recede;
    vAcross = side;
}`;
const LINE_FS = COMMON + /* glsl */ `
in vec3 vGlow;
in float vAcross;
out vec4 color;
void main() {
    float d = abs(vAcross);
    float core = exp(-d * d * 18.0);
    float halo = exp(-d * d * 3.0) * 0.35;
    color = vec4(vGlow * (core * 1.3 + halo), 0.0);
}`;

const GATE_VS = COMMON + /* glsl */ `
out vec2 vQ;
void main() {
    vec2 q = vec2(gl_VertexID & 1, gl_VertexID >> 1);
    float z = (q.x - 0.5) * (BAND_SPAN + 1.6);
    float y = mix(-0.12, 2.0, q.y);
    gl_Position = uViewProj * vec4(0.0, y, z, 1.0);
    vQ = q;
}`;
const GATE_FS = COMMON + /* glsl */ `
in vec2 vQ;
out vec4 color;
void main() {
    float e = uFilter.x;
    vec2 q = vQ;
    vec2 size = vec2(BAND_SPAN + 1.6, 2.5);
    vec2 edge = min(q, 1.0 - q) * size;
    // A hairline frame, brightest along the base where the waves cross.
    float frame = exp(-min(edge.x, edge.y) * 38.0) * (0.55 + 0.45 * (1.0 - q.y));
    // The aperture: the passband's own response curve, drawn as a lit slot.
    float z = (q.x - 0.5) * size.x;
    float i = (z / BAND_SPAN + 0.5) * (uField.x - 1.0);
    float g = gain(i, -uFilter.y * uEye.w);
    float slotEdge = exp(-sq((g - 0.5) * 7.0));
    float slot = g * 0.10 * (1.0 - q.y) + slotEdge * 0.55;
    float glass = 0.024 * (1.0 - q.y * 0.7);
    float lit = 0.1 + 0.9 * e;
    color = vec4((vec3(0.35, 0.62, 1.0) * (frame + glass) + vec3(0.62, 0.82, 1.0) * slot) * lit, 0.0);
}`;

// MARK: - renderer

export type Squelch = {
  /// 0..1 through the story: drives the gate and the camera pull-back.
  setProgress(p: number): void;
  /// Stop drawing (offscreen) without tearing anything down.
  setVisible(v: boolean): void;
  destroy(): void;
};

/// Returns null when WebGL2 is unavailable or a shader fails to build, and the
/// caller keeps its poster.
export function createSquelch(
  canvas: HTMLCanvasElement,
  opts: { reduceMotion: boolean },
): Squelch | null {
  const gl = canvas.getContext("webgl2", {
    // 4x MSAA on the default framebuffer: without it the edges where near
    // bands hide far ones crawl as the field scrolls.
    antialias: true,
    alpha: false,
    depth: true,
    powerPreference: "high-performance",
  });
  if (!gl) return null;

  const compile = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
      console.warn("[squelch]", gl.getShaderInfoLog(s));
      return null;
    }
    return s;
  };
  const program = (vs: string, fs: string) => {
    const v = compile(gl.VERTEX_SHADER, vs);
    const f = compile(gl.FRAGMENT_SHADER, fs);
    if (!v || !f) return null;
    const p = gl.createProgram()!;
    gl.attachShader(p, v);
    gl.attachShader(p, f);
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
      console.warn("[squelch]", gl.getProgramInfoLog(p));
      return null;
    }
    return p;
  };

  const programs = {
    bg: program(BG_VS, BG_FS),
    curtain: program(CURTAIN_VS, CURTAIN_FS),
    line: program(LINE_VS, LINE_FS),
    gate: program(GATE_VS, GATE_FS),
  };
  if (!programs.bg || !programs.curtain || !programs.line || !programs.gate) return null;

  // Attribute-less draws still want a bound VAO in some drivers.
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);

  // Phones get half the segments: the curve still reads, the vertex shader
  // (where all the noise lives) does half the work.
  const segments = () => (Math.min(innerWidth, innerHeight) < 700 ? 700 : 1400);

  const state = new SceneState();
  let progress = 0;
  let visible = true;
  let raf = 0;
  let last = 0;
  let width = 1;
  let height = 1;

  const resize = () => {
    const dpr = Math.min(window.devicePixelRatio || 1, innerWidth < 700 ? 1.5 : 2);
    const rect = canvas.getBoundingClientRect();
    width = Math.max(1, Math.round(rect.width * dpr));
    height = Math.max(1, Math.round(rect.height * dpr));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
  };

  const draw = () => {
    const segs = segments();
    const aspect = width / height;
    const s = state;
    // Close and low in the noise, pulling up and back to frame the exit.
    const k = s.camera * s.camera * (3 - 2 * s.camera);
    const drift: Vec3 = [Math.sin(s.time * 0.07) * 0.6, Math.sin(s.time * 0.05) * 0.25, 0];
    const lerp = (a: Vec3, b: Vec3): Vec3 => [
      a[0] + (b[0] - a[0]) * k,
      a[1] + (b[1] - a[1]) * k,
      a[2] + (b[2] - a[2]) * k,
    ];
    const e0 = lerp([4.2, 2.6, 8.4], [9.4, 5.0, 12.4]);
    const eye: Vec3 = [e0[0] + drift[0], e0[1] + drift[1], e0[2] + drift[2]];
    const target = lerp([-1.6, 0.1, 0], [2.6, -0.1, 0.6]);
    // Wide windows get the scene pushed right so the copy has room. Tall
    // screens widen the lens to keep roughly the same horizontal field, and
    // push the scene down under the copy.
    const fovY = aspect < 1.2 ? 2 * Math.atan((Math.tan(0.31) * 0.78) / aspect) : 0.62;
    const lens = perspective(fovY, aspect, 0.1, 80);
    const shift = translation(aspect > 1.3 ? 0.28 : 0, aspect > 1.3 ? 0 : aspect < 0.8 ? -0.42 : -0.22);
    const viewProj = multiply(shift, multiply(lens, lookAt(eye, target)));

    const set = (p: WebGLProgram) => {
      gl.useProgram(p);
      gl.uniformMatrix4fv(gl.getUniformLocation(p, "uViewProj"), false, viewProj);
      gl.uniform4f(gl.getUniformLocation(p, "uEye"), eye[0], eye[1], eye[2], s.time);
      gl.uniform4f(gl.getUniformLocation(p, "uFrame"), width, height, s.filterLo, s.filterHi);
      gl.uniform4f(gl.getUniformLocation(p, "uField"), BANDS, segs, X_MIN, X_MAX);
      gl.uniform4f(gl.getUniformLocation(p, "uFilter"), s.engage, WAVE_SPEED, 0, 0);
    };

    gl.viewport(0, 0, width, height);
    gl.clearColor(0, 0, 0, 1);
    gl.clearDepth(1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    const strip = (segs + 1) * 2;

    gl.disable(gl.BLEND);
    gl.disable(gl.DEPTH_TEST);
    set(programs.bg!);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.depthMask(true);
    set(programs.curtain!);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, strip, BANDS);

    // Additive from here on, depth tested, never depth writing.
    gl.depthMask(false);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE);
    set(programs.gate!);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

    set(programs.line!);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, strip, BANDS);
  };

  // The gate closes a little into the scroll, not at its very top, so the
  // noise beat gets a moment of its own before anything happens.
  const ENGAGE_AT = 0.22;
  const cameraFor = (p: number) => {
    const t = Math.min(1, Math.max(0, (p - ENGAGE_AT) / 0.6));
    return t * t * (3 - 2 * t);
  };

  const loop = (now: number) => {
    const dt = last ? Math.min(0.05, (now - last) / 1000) : 1 / 60;
    last = now;
    state.step(dt, progress > ENGAGE_AT, cameraFor(progress));
    resize();
    draw();
    raf = visible ? requestAnimationFrame(loop) : 0;
  };

  // Reduced motion: one still frame, held on the resolved passband. Settled by
  // stepping the state forward, never by animating it on screen.
  const still = () => {
    const s = new SceneState();
    for (let i = 0; i < 60 * 9; i++) s.step(1 / 60, i > 60 * 2, 1);
    Object.assign(state, s);
    resize();
    draw();
  };

  const onResize = () => (opts.reduceMotion ? still() : resize());
  addEventListener("resize", onResize);

  if (opts.reduceMotion) still();
  else raf = requestAnimationFrame(loop);

  return {
    setProgress(p) {
      progress = p;
    },
    setVisible(v) {
      if (v === visible) return;
      visible = v;
      if (v && !opts.reduceMotion && !raf) {
        last = 0;
        raf = requestAnimationFrame(loop);
      }
    },
    destroy() {
      cancelAnimationFrame(raf);
      removeEventListener("resize", onResize);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    },
  };
}
