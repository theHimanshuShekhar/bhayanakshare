// Takes a PNG of every view of the screen previews into ui/.screens-out/ (not committed).
//
//   pnpm --filter ui capture:screens            every view at 1100x760, key ones also at 640 wide
//   pnpm --filter ui capture:screens home-populated offer-warnings     only these views
//
// It uses the preview server if one is already running (`pnpm --filter ui preview:screens`),
// and starts its own otherwise. Needs Chromium: `pnpm --filter ui exec playwright install chromium`.

import { mkdir, readdir, rm, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { createServer } from "vite";

const here = (path) => fileURLToPath(new URL(path, import.meta.url));
const OUT = here("../.screens-out");
const URL_BASE = "http://localhost:1430/";
/** The window, and the same window at 200% zoom (1280 px wide at 200% is 640 px). */
const WIDE = { width: 1100, height: 760 };
const NARROW = { width: 640, height: 760 };

const only = new Set(process.argv.slice(2));

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

// The camera view reads a stand-in camera that Chromium supplies, so the scanner has a picture.
const browser = await chromium.launch({
  args: ["--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream"],
});
const context = await browser.newContext({
  viewport: WIDE,
  locale: "en-US",
  timezoneId: "UTC",
  permissions: ["camera"],
});
// The same moment History is dated from (sample.ts), so an Offer's countdown reads the same in every capture.
await context.clock.setFixedTime(new Date("2026-10-10T12:00:00Z"));
const failures = [];

try {
  // The index says which views there are and which are also captured narrow.
  const index = await context.newPage();
  await index.goto(URL_BASE, { waitUntil: "networkidle" });
  const views = await index.$$eval("a[data-view]", (links) =>
    links.map((a) => ({ id: a.dataset.view, narrow: a.hasAttribute("data-narrow"), clip: a.dataset.clip })),
  );
  await index.close();
  const wanted = views.filter((v) => only.size === 0 || only.has(v.id));
  if (wanted.length === 0) throw new Error(`No view matches: ${[...only].join(", ")}`);

  // A full run starts clean; a run of some views leaves the others as they were.
  if (only.size === 0) await rm(OUT, { recursive: true, force: true });
  await mkdir(OUT, { recursive: true });

  const shoot = async (view, size, name) => {
    const page = await context.newPage();
    await page.setViewportSize(size);
    page.on("pageerror", (e) => failures.push(`${view.id}: ${e.message}`));
    try {
      await page.goto(`${URL_BASE}?view=${view.id}`);
      await page.waitForFunction(() => document.documentElement.dataset.screen !== undefined, null, {
        timeout: 30_000,
      });
      const state = await page.evaluate(() => ({ ...document.documentElement.dataset }));
      if (state.screen !== "ready") throw new Error(state.error || "the view did not get ready");
      // The page's own fonts and layout have settled.
      await page.evaluate(() => document.fonts.ready);
      // Focus rings show where the keyboard is, and nobody is typing.
      await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
      const path = `${OUT}/${name}.png`;
      if (view.clip) {
        await page.locator(view.clip).first().screenshot({ path });
      } else {
        // A sheet sits over the window, so it is shot as the window; a page is shot whole.
        const sheet = (await page.locator("[role=dialog], [role=alertdialog]").count()) > 0;
        await page.screenshot({ path, fullPage: !sheet });
      }
      console.log(`ok   ${name}.png`);
    } catch (e) {
      failures.push(`${view.id}: ${e.message}`);
      console.log(`FAIL ${name}: ${e.message}`);
    } finally {
      await page.close();
    }
  };

  for (const view of wanted) {
    await shoot(view, WIDE, view.id);
    if (view.narrow) await shoot(view, NARROW, `${view.id}.narrow`);
  }

  const files = (await readdir(OUT)).filter((f) => f.endsWith(".png")).sort();
  console.log(`\n${files.length} captures in ${OUT}`);
  for (const f of files) console.log(`${String((await stat(`${OUT}/${f}`)).size).padStart(8)}  ${f}`);
} finally {
  await browser.close();
  await server?.close();
}

if (failures.length > 0) {
  console.error(`\n${failures.length} problem(s):\n${failures.join("\n")}`);
  process.exit(1);
}
