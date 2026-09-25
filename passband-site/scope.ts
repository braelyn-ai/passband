// The hero: a phosphor spectrum analyzer pointed at an inbox.
//
// Deliberately NOT the app's cold open. That one is a 3D waterfall and it is
// the app's to show the first time it launches; the site says the same thing
// in a different instrument's language, flat and 2D, so the download still
// has a moment of its own.
//
// x is "frequency", y is energy. At rest the trace is a noise floor full of
// transient spikes, with three clean carriers buried in it: the mail that
// matters. Scrolling closes the squelch: an 8th-order filter curve (the same
// shape the waitlist meter draws) narrows from wide open down to one band,
// everything outside its skirts flattens, and the carriers stand clear.
//
// Canvas 2D, no WebGL: one trace is cheap, it runs everywhere, and a fallback
// poster stops being a thing to maintain. Persistence is the whole look: every
// frame fades what was there rather than clearing it, so the trace leaves the
// afterglow a real phosphor screen does.

export type ScopeLayout = {
  /// Where the passband is tuned, 0..1 across the canvas.
  center: number;
  /// The trace's baseline and its full-scale height, as fractions of the canvas.
  base: number;
  scale: number;
  /// How far apart the carriers sit and how wide the band closes, as a
  /// multiple of the desktop spacing. A phone needs more of both to read.
  spread: number;
};

export type Scope = {
  setProgress(p: number): void;
  setVisible(v: boolean): void;
  destroy(): void;
};

const POINTS = 520;

// The carriers, as offsets from the band's centre, with their heights. They
// sit at the same height before and after: the squelch does not make the mail
// that matters louder, it takes everything else away.
export const CARRIERS = [
  { dx: -0.052, h: 0.56 },
  { dx: 0.004, h: 0.7 },
  { dx: 0.047, h: 0.46 },
];

const BG = "9, 13, 22";

// The same passband the meter and the app's gate use: flat top, steep skirts.
const response = (x: number, c: number, w: number) => 1 / Math.sqrt(1 + Math.pow((x - c) / w, 16));

const ease = (t: number) => {
  const k = Math.min(1, Math.max(0, t));
  return k * k * (3 - 2 * k);
};

/// Where a carrier's marker goes, in canvas fractions, for the DOM overlay.
export function markerPoint(i: number, layout: ScopeLayout) {
  const c = CARRIERS[i];
  return { x: layout.center + c.dx * layout.spread, y: layout.base - c.h * layout.scale };
}

export function createScope(
  canvas: HTMLCanvasElement,
  opts: {
    reduceMotion: boolean;
    layout: () => ScopeLayout;
    onReadout: (squelch: number) => void;
  },
): Scope | null {
  const ctx = canvas.getContext("2d", { alpha: false });
  if (!ctx) return null;

  let width = 1;
  let height = 1;
  let dpr = 1;
  let progress = 0;
  let engage = 0;
  let visible = true;
  let raf = 0;
  let last = 0;
  let time = 0;

  // The displayed trace, averaged frame to frame the way an analyzer's video
  // filter does, so the noise reads as hiss rather than strobing.
  const trace = new Float32Array(POINTS);

  // Transient spikes: the notifications, digests and offers. Each one flares
  // somewhere random and decays; a dead one is reborn elsewhere.
  type Spike = { x: number; h: number; w: number; life: number; decay: number };
  const spawn = (s?: Spike): Spike => {
    const o = s ?? ({} as Spike);
    o.x = Math.random();
    o.h = 0.18 + Math.random() * 0.5;
    o.w = 0.003 + Math.random() * 0.008;
    o.life = 1;
    o.decay = 0.6 + Math.random() * 1.8;
    return o;
  };
  const spikes: Spike[] = Array.from({ length: 22 }, () => {
    const s = spawn();
    s.life = Math.random();
    return s;
  });

  // Slow humps under the floor, wandering on incommensurate beats.
  const humps = [
    { f: 0.07, p: 0.3, a: 0.16 },
    { f: 0.05, p: 2.1, a: 0.12 },
    { f: 0.11, p: 4.0, a: 0.1 },
  ];

  const resize = () => {
    dpr = Math.min(window.devicePixelRatio || 1, innerWidth < 700 ? 2 : 1.75);
    const rect = canvas.getBoundingClientRect();
    width = Math.max(1, Math.round(rect.width * dpr));
    height = Math.max(1, Math.round(rect.height * dpr));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
      ctx.fillStyle = `rgb(${BG})`;
      ctx.fillRect(0, 0, width, height);
    }
  };

  // One sample of the spectrum at x, before averaging.
  const sample = (x: number, e: number, c: number, w: number, spread: number, still: boolean) => {
    const floor = 0.035 + (still ? 0.02 : Math.random() * 0.045);
    let noise = floor;
    for (const h of humps) {
      const hx = 0.5 + 0.42 * Math.sin(time * h.f + h.p);
      noise += h.a * Math.exp(-(((x - hx) / 0.09) ** 2));
    }
    for (const s of spikes) {
      const d = (x - s.x) / s.w;
      if (d > -4 && d < 4) noise += s.h * s.life * Math.exp(-d * d);
    }
    // Inside the band the floor survives, quieter; outside it, nothing does.
    const g = response(x, c, w);
    noise *= 1 - e * (1 - g * 0.2);
    let signal = 0;
    for (const k of CARRIERS) {
      const d = (x - (c + k.dx * spread)) / (0.0045 * spread);
      if (d > -6 && d < 6) {
        const shimmer = still ? 1 : 0.94 + 0.06 * Math.sin(time * 7 + k.dx * 300);
        signal += k.h * shimmer * (1 / (1 + d * d));
      }
    }
    return Math.max(noise, signal) + Math.min(noise, signal) * 0.3;
  };

  const draw = (dt: number, still: boolean) => {
    const L = opts.layout();
    const e = engage;
    const c = L.center;
    // Wide open is a curve flat across the whole screen; closed is one band.
    const w = 1.6 * Math.pow((0.085 * L.spread) / 1.6, ease(e));
    const baseY = L.base * height;
    const amp = L.scale * height;

    // Persistence: fade, never clear. Faster when still so a resize settles.
    ctx.globalCompositeOperation = "source-over";
    ctx.fillStyle = `rgba(${BG}, ${still ? 1 : Math.min(1, 0.2 * dt * 60)})`;
    ctx.fillRect(0, 0, width, height);

    // The filter's response, drawn as the gate: a shaded mask under a thin
    // line, only once it has started to close.
    const gate = Math.min(1, e * 3);
    if (gate > 0.01) {
      const top = (x: number) => baseY - (0.02 + 0.84 * response(x, c, w)) * amp;
      ctx.beginPath();
      ctx.moveTo(0, baseY);
      for (let i = 0; i <= 200; i++) ctx.lineTo((i / 200) * width, top(i / 200));
      ctx.lineTo(width, baseY);
      ctx.closePath();
      const fill = ctx.createLinearGradient(0, baseY - amp, 0, baseY);
      fill.addColorStop(0, `rgba(78, 155, 234, ${0.07 * gate})`);
      fill.addColorStop(1, `rgba(78, 155, 234, 0)`);
      ctx.fillStyle = fill;
      ctx.fill();
      ctx.beginPath();
      for (let i = 0; i <= 200; i++) {
        const x = (i / 200) * width;
        if (i === 0) ctx.moveTo(x, top(0));
        else ctx.lineTo(x, top(i / 200));
      }
      ctx.setLineDash([4 * dpr, 5 * dpr]);
      ctx.strokeStyle = `rgba(130, 186, 245, ${0.5 * gate})`;
      ctx.lineWidth = 1 * dpr;
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // The trace, averaged, then stroked three times additively: a wide dim
    // halo, a glow, a near-white core. Cheaper than shadowBlur and prettier.
    const k = still ? 1 : 1 - Math.exp(-dt * 22);
    for (let i = 0; i < POINTS; i++) {
      const x = i / (POINTS - 1);
      trace[i] += (sample(x, e, c, w, L.spread, still) - trace[i]) * k;
    }
    ctx.globalCompositeOperation = "lighter";
    const path = new Path2D();
    for (let i = 0; i < POINTS; i++) {
      const x = (i / (POINTS - 1)) * width;
      const y = baseY - Math.min(1, trace[i]) * amp;
      if (i === 0) path.moveTo(x, y);
      else path.lineTo(x, y);
    }
    ctx.lineJoin = "round";
    const strokes: Array<[number, string]> = [
      [9, "rgba(78, 155, 234, 0.05)"],
      [3.5, "rgba(78, 155, 234, 0.28)"],
      [1.2, "rgba(214, 233, 255, 0.85)"],
    ];
    for (const [lw, color] of strokes) {
      ctx.lineWidth = lw * dpr;
      ctx.strokeStyle = color;
      ctx.stroke(path);
    }
    ctx.globalCompositeOperation = "source-over";
  };

  const step = (dt: number) => {
    time += dt;
    const target = ease((progress - 0.12) / 0.55);
    engage += (target - engage) * (1 - Math.exp(-dt * 4));
    for (const s of spikes) {
      s.life -= dt * s.decay;
      if (s.life <= 0) spawn(s);
    }
    opts.onReadout(engage);
  };

  const loop = (now: number) => {
    const dt = last ? Math.min(0.05, (now - last) / 1000) : 1 / 60;
    last = now;
    step(dt);
    resize();
    draw(dt, false);
    raf = visible ? requestAnimationFrame(loop) : 0;
  };

  // Reduced motion: one settled frame at the current progress, redrawn when
  // the scroll moves rather than animated.
  const still = () => {
    engage = ease((progress - 0.12) / 0.55);
    opts.onReadout(engage);
    resize();
    draw(1 / 60, true);
  };

  const onResize = () => (opts.reduceMotion ? still() : resize());
  addEventListener("resize", onResize);
  if (opts.reduceMotion) still();
  else raf = requestAnimationFrame(loop);

  return {
    setProgress(p) {
      progress = p;
      if (opts.reduceMotion) still();
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
    },
  };
}
