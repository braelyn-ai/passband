// Screenshots card.html in the system Chrome. Playwright rather than
// `chrome --headless --screenshot`: that one's virtual clock never runs the
// animation frames, so the squelch is still wide open when it shoots.
import { chromium } from "playwright-core";

const [dir, out] = process.argv.slice(2);
const server = Bun.serve({
  port: 0,
  fetch: (req) => {
    const file = Bun.file(`${dir}${new URL(req.url).pathname}`);
    return file.size ? new Response(file) : new Response(null, { status: 404 });
  },
});
const browser = await chromium.launch({
  executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
});
const page = await browser.newPage({ viewport: { width: 1200, height: 630 } });
await page.goto(`http://localhost:${server.port}/card.html`);
await page.waitForFunction(() => (window as any).ready, null, { timeout: 15000 });
await page.screenshot({ path: out });
await browser.close();
server.stop();
