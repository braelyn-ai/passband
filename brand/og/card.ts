// The link-unfurl card's live part: the landing page's own analyzer, closed on
// its three carriers, with the markers riding their peaks. Imported from the
// site rather than redrawn so the card and the hero are one trace. It runs
// animated, not reduced-motion: the glow is persistence, built up over frames,
// and a single settled frame is a thin line.
import { createScope, markerPoint, type ScopeLayout } from "../../passband-site/scope";

const layout: ScopeLayout = { center: 0.5, base: 0.92, scale: 0.3, spread: 1.4 };

const canvas = document.querySelector("canvas")!;
const scope = createScope(canvas, { reduceMotion: false, layout: () => layout, onReadout: () => {} })!;
scope.setProgress(1);

// The squelch eases shut over a second or so; shoot.ts waits for this.
setTimeout(() => ((window as any).ready = true), 3500);

document.querySelectorAll<HTMLElement>(".mkr").forEach((el, i) => {
  const { x, y } = markerPoint(i, layout);
  el.style.left = `${x * 100}%`;
  el.style.top = `${y * 100}%`;
});
