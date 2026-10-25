// Takes the README's screenshots into docs/screenshots/ (committed): a few views of the screen
// previews, as the window shows them, at twice the pixels so they stay sharp on a dense display.
//
//   pnpm --filter ui capture:readme
//
// Like capture.mjs it uses the preview server if one is running and starts its own otherwise, and
// it needs Chromium: `pnpm --filter ui exec playwright install chromium`.

import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { createServer } from "vite";

const here = (path) => fileURLToPath(new URL(path, import.meta.url));
const OUT = here("../../docs/screenshots");
const URL_BASE = "http://localhost:1430/";

/** Each screenshot: the view, its file name, the theme, and the window it is taken in, or the part
 * of the page (`clip`) it is cut to. */
const SHOTS = [
  { view: "transfers-in-progress", file: "home", theme: "light", width: 1100, height: 790 },
  { view: "offer-files", file: "offer", theme: "light", width: 1100, height: 760 },
  { view: "batch-expanded", file: "batch", theme: "light", clip: "section[aria-labelledby=transfers-heading]" },
  { view: "dialog-add-contact-check", file: "fingerprint", theme: "dark", width: 1100, height: 760 },
  { view: "my-device-id", file: "my-id", theme: "dark", clip: "section[aria-labelledby=my-id-heading]" },
  { view: "history-populated", file: "history", theme: "dark", width: 1100, height: 900 },
  { view: "settings", file: "privacy", theme: "light", clip: "section[aria-labelledby=settings-privacy-heading]" },
];

async function running() {
  try {
    return (await fetch(URL_BASE)).ok;
  } catch {
    return false;
  }
}

const server = (await running())
  ? null
  : await createServer({ root: here("."), configFile: here("./vite.config.ts"), logLevel: "warn" }).then((s) => s.listen());

const browser = await chromium.launch();
let failed = false;
try {
  await mkdir(OUT, { recursive: true });
  for (const shot of SHOTS) {
    const context = await browser.newContext({
      viewport: { width: shot.width ?? 1100, height: shot.height ?? 760 },
      deviceScaleFactor: 2,
      colorScheme: shot.theme,
      locale: "en-US",
      timezoneId: "UTC",
    });
    // The same moment the sample data is dated from, as in capture.mjs.
    await context.clock.setFixedTime(new Date("2026-10-10T12:00:00Z"));
    const page = await context.newPage();
    try {
      await page.goto(`${URL_BASE}?view=${shot.view}`);
      await page.waitForFunction(() => document.documentElement.dataset.screen !== undefined, null, { timeout: 30_000 });
      const state = await page.evaluate(() => ({ ...document.documentElement.dataset }));
      if (state.screen !== "ready") throw new Error(state.error || "the view did not get ready");
      await page.evaluate(() => document.fonts.ready);
      await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
      const path = `${OUT}/${shot.file}.png`;
      if (shot.clip) await page.locator(shot.clip).first().screenshot({ path });
      else await page.screenshot({ path });
      console.log(`ok   ${shot.file}.png`);
    } catch (e) {
      failed = true;
      console.log(`FAIL ${shot.file}: ${e.message}`);
    } finally {
      await context.close();
    }
  }
} finally {
  await browser.close();
  await server?.close();
}
if (failed) process.exit(1);
