import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { join } from "node:path";
import { expect, test, type Browser, type BrowserContext, type Page } from "@playwright/test";
import type { Boot } from "../harness/boot";
import { MOCKUPS, NOW, SCREENSHOTS, emitAll, emitEvent, open, placeBar, placePanel } from "./support";

/**
 * Deterministic browser simulation of the overlay and control window at 100% and 150% display scaling,
 * next to the mockup boards rendered offline (their Google Fonts requests are blocked, so the boards fall back
 * to the browser default sans-serif). Windows system fonts are used for the app; real Windows DPI is not proven.
 */
const SCALES = [1, 1.5] as const;
const pct = (scale: number): string => `${Math.round(scale * 100)}`;

async function context(browser: Browser, scale: number, size: { width: number; height: number }, scheme: "light" | "dark" = "light"): Promise<BrowserContext> {
  return browser.newContext({
    viewport: size,
    deviceScaleFactor: scale,
    reducedMotion: "reduce",
    colorScheme: scheme,
    locale: "en-US",
  });
}

async function mockup(browser: Browser, file: string, query: string, scale: number, size: { width: number; height: number }, out: string): Promise<string> {
  const ctx = await context(browser, scale, size);
  const page = await ctx.newPage();
  await page.route(/^https?:/, (route) => route.abort());
  await page.goto(`${pathToFileURL(join(MOCKUPS, file)).href}${query}`);
  await page.waitForSelector("x-dc > div, body > div");
  const path = join(SCREENSHOTS, out);
  await page.screenshot({ path, animations: "disabled" });
  await ctx.close();
  return path;
}

function pngSize(path: string): { width: number; height: number } {
  const data = readFileSync(path);
  return { width: data.readUInt32BE(16), height: data.readUInt32BE(20) };
}

async function sideBySide(browser: Browser, scale: number, appPath: string, mockPath: string, out: string, note: string): Promise<void> {
  const app = pngSize(appPath);
  const mock = pngSize(mockPath);
  const width = Math.max(app.width, mock.width) / scale;
  const ctx = await browser.newContext({ viewport: { width: Math.ceil(width), height: 400 }, deviceScaleFactor: scale });
  const page = await ctx.newPage();
  const data = (path: string) => `data:image/png;base64,${readFileSync(path).toString("base64")}`;
  await page.setContent(`
    <style>
      body { margin: 0; padding: 12px 0 12px; background: #888; font: 13px "Segoe UI", sans-serif; color: #111; }
      figure { margin: 0 0 14px; }
      figcaption { padding: 0 12px 6px; }
      img { display: block; width: ${app.width / scale}px; }
      img.mock { width: ${mock.width / scale}px; }
    </style>
    <figure><figcaption>APP (browser simulation, ${pct(scale)}% scale, system fonts): ${note}</figcaption><img src="${data(appPath)}"></figure>
    <figure><figcaption>MOCKUP board rendered offline (fonts fall back, board chrome differs): ${note}</figcaption><img class="mock" src="${data(mockPath)}"></figure>
  `);
  await page.screenshot({ path: join(SCREENSHOTS, out), fullPage: true });
  await ctx.close();
}

async function barScene(page: Page, boot: Boot = {}): Promise<void> {
  await page.clock.install({ time: 0 });
  await open(page, "overlay", { ...boot, state: { listening: "starting", ...boot.state } });
  await placeBar(page);
  await emitEvent(page, { type: "source_changed", info: { mode: "system", label: "system audio", sample_rate: 48000, channels: 2 } });
  await emitAll(page, [
    NOW.asr(1, "这波操作真的太秀了，我直接看傻了。"),
    NOW.final(1, "This move is really amazing; I'm completely amazed."),
    NOW.asr(2, "大家好，欢迎来到我的直播间，今天我们继续打这个副本。"),
    NOW.delta(2, "Hello everyone, welcome to my live stream. Today, we continue"),
    { type: "listening_state", state: "listening" },
  ]);
  await page.clock.pauseAt(1000);
}

async function panelScene(page: Page, boot: Boot = {}): Promise<void> {
  await page.clock.install({ time: 0 });
  await open(page, "overlay", { ...boot, config: { overlay: { style: "panel", background: 0.86 }, ...boot.config } });
  await placePanel(page);
  await emitEvent(page, { type: "source_changed", info: { mode: "apps", label: "Chrome, VLC", sample_rate: 48000, channels: 2 } });
  await emitAll(page, [
    NOW.asr(1, "等一下，我麦克风是不是没开？"),
    NOW.final(1, "Wait, is my mic off?"),
    NOW.asr(2, "OK, can you guys hear me now?", "english", "en"),
    NOW.asr(3, "这个老板太难打了，我已经死了十几次了。"),
    NOW.final(3, "This boss is way too hard. I've already died more than ten times."),
    NOW.asr(4, "大家好，欢迎来到我的直播间，今天我们继续打这个副本。"),
    NOW.delta(4, "Hello everyone, welcome to my live stream. Today, we continue"),
  ]);
  await page.clock.pauseAt(1000);
}

test.describe("screenshots (browser simulation)", () => {
  for (const scale of SCALES) {
    test(`subtitle bar at ${pct(scale)}%`, async ({ browser }) => {
      const size = { width: 1280, height: 720 };
      const ctx = await context(browser, scale, size);
      const page = await ctx.newPage();
      await barScene(page);
      await expect(page.locator(".caret")).toBeVisible();
      const app = join(SCREENSHOTS, `overlay-bar-${pct(scale)}.png`);
      await page.screenshot({ path: app, animations: "disabled" });
      await ctx.close();
      const mock = await mockup(browser, "Main.dc.html", "", scale, size, `mockup-overlay-bar-${pct(scale)}.png`);
      await sideBySide(browser, scale, app, mock, `side-by-side-overlay-bar-${pct(scale)}.png`, "subtitle bar");
    });

    test(`subtitle bar move mode at ${pct(scale)}%`, async ({ browser }) => {
      const size = { width: 1280, height: 720 };
      const ctx = await context(browser, scale, size);
      const page = await ctx.newPage();
      await barScene(page, { state: { overlay_moving: true } });
      await expect(page.getByTestId("move-frame")).toBeVisible();
      const app = join(SCREENSHOTS, `overlay-bar-move-${pct(scale)}.png`);
      await page.screenshot({ path: app, animations: "disabled" });
      await ctx.close();
      const mock = await mockup(browser, "Main.dc.html", "?mode=Move", scale, size, `mockup-overlay-bar-move-${pct(scale)}.png`);
      await sideBySide(browser, scale, app, mock, `side-by-side-overlay-bar-move-${pct(scale)}.png`, "move mode");
    });

    test(`caption panel at ${pct(scale)}%`, async ({ browser }) => {
      const size = { width: 1280, height: 720 };
      const ctx = await context(browser, scale, size);
      const page = await ctx.newPage();
      await panelScene(page);
      const plain = join(SCREENSHOTS, `overlay-panel-${pct(scale)}.png`);
      await page.screenshot({ path: plain, animations: "disabled" });
      await page.getByTestId("caption-panel").hover();
      await expect(page.getByTestId("panel-header")).toBeVisible();
      const hover = join(SCREENSHOTS, `overlay-panel-hover-${pct(scale)}.png`);
      await page.screenshot({ path: hover, animations: "disabled" });
      await ctx.close();
      const mockPlain = await mockup(browser, "OverlayPanel.dc.html", "?hover=false", scale, size, `mockup-overlay-panel-${pct(scale)}.png`);
      const mockHover = await mockup(browser, "OverlayPanel.dc.html", "", scale, size, `mockup-overlay-panel-hover-${pct(scale)}.png`);
      await sideBySide(browser, scale, plain, mockPlain, `side-by-side-overlay-panel-${pct(scale)}.png`, "caption panel");
      await sideBySide(browser, scale, hover, mockHover, `side-by-side-overlay-panel-hover-${pct(scale)}.png`, "caption panel, hover header");
    });

    test(`caption line states at ${pct(scale)}%`, async ({ browser }) => {
      const size = { width: 1280, height: 760 };
      const ctx = await context(browser, scale, size);
      const page = await ctx.newPage();
      await page.goto("/e2e/harness/states.html");
      await page.waitForSelector(".board");
      await expect(page.locator("article")).toHaveCount(7);
      const app = join(SCREENSHOTS, `caption-states-${pct(scale)}.png`);
      await page.screenshot({ path: app, animations: "disabled" });
      await ctx.close();
      const mock = await mockup(browser, "CaptionStates.dc.html", "", scale, size, `mockup-caption-states-${pct(scale)}.png`);
      await sideBySide(browser, scale, app, mock, `side-by-side-caption-states-${pct(scale)}.png`, "caption line states");
    });
  }

  const size = { width: 900, height: 580 };
  const mockSize = { width: 900, height: 620 };

  for (const scheme of ["light", "dark"] as const) {
    for (const scale of SCALES) {
      test(`control window pages, ${scheme}, ${pct(scale)}%`, async ({ browser }) => {
        const ctx = await context(browser, scale, size, scheme);
        const page = await ctx.newPage();
        const external = await open(page, "control", { config: { capture: { mode: "system" } } });
        await expect(page.getByTestId("header-title")).toHaveText("Listening");
        const shoot = async (name: string): Promise<string> => {
          const path = join(SCREENSHOTS, `control-${name}-${scheme}-${pct(scale)}.png`);
          await page.screenshot({ path, animations: "disabled" });
          return path;
        };
        const capture = await shoot("capture-system");
        for (const [role, name] of [["radio", /Selected apps/], ["checkbox", /Google Chrome/]] as const) {
          await page.getByRole(role, { name }).click();
          await expect(page.getByRole(role, { name })).toBeChecked();
        }
        await expect(page.locator(".list-row").first()).toContainText("-6 dB");
        const apps = await shoot("capture-apps");
        await page.getByRole("tab", { name: "Overlay" }).click();
        const overlay = await shoot("overlay");
        await page.getByRole("tab", { name: "Languages" }).click();
        const languages = await shoot("languages");
        await page.getByRole("tab", { name: "Performance" }).click();
        const performance = await shoot("performance");
        expect(external).toEqual([]);
        await ctx.close();

        if (scheme === "light") {
          const boards: [string, string, string, string][] = [
            [capture, "?tab=capture&source=system", "capture-system", "capture, whole system"],
            [apps, "?tab=capture&source=apps", "capture-apps", "capture, selected apps"],
            [overlay, "?tab=overlay", "overlay", "overlay page"],
            [languages, "?tab=lang", "languages", "languages page"],
            [performance, "?tab=perf", "performance", "performance page"],
          ];
          for (const [appPath, query, name, note] of boards) {
            const mock = await mockup(browser, "Controls.dc.html", query, scale, mockSize, `mockup-control-${name}-${pct(scale)}.png`);
            await sideBySide(browser, scale, appPath, mock, `side-by-side-control-${name}-${pct(scale)}.png`, note);
          }
        }
      });
    }
  }

  for (const scale of SCALES) {
    test(`first run at ${pct(scale)}%`, async ({ browser }) => {
      const MB = 1024 * 1024;
      const ctx = await context(browser, scale, size);
      const page = await ctx.newPage();
      await open(page, "control", {
        state: { models_ready: false, listening: "paused" },
        models: [
          { id: "vad", name: "Voice detection", bytes_total: 2 * MB, bytes_done: 2 * MB, state: "ready" },
          { id: "asr", name: "Speech recognition", bytes_total: 239 * MB, bytes_done: 152 * MB, state: "downloading" },
          { id: "mt", name: "Translation", bytes_total: 1106 * MB, bytes_done: 0, state: "missing" },
        ],
      });
      await expect(page.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "64");
      const app = join(SCREENSHOTS, `first-run-${pct(scale)}.png`);
      await page.screenshot({ path: app, animations: "disabled" });
      await ctx.close();
      const mock = await mockup(browser, "FirstRun.dc.html", "", scale, mockSize, `mockup-first-run-${pct(scale)}.png`);
      await sideBySide(browser, scale, app, mock, `side-by-side-first-run-${pct(scale)}.png`, "first run (the app shows no download speed: the DTO has none)");
    });
  }

  test("writes an index of the artifacts", async () => {
    writeFileSync(
      join(SCREENSHOTS, "README.txt"),
      [
        "Browser simulation of the UI (headless Chromium via Playwright, mocked Tauri transport, synthetic data).",
        "It is NOT a native Windows run: WebView2 rendering, click-through, focus, Alt-Tab and real Windows DPI are untested here.",
        "100% = deviceScaleFactor 1, 150% = deviceScaleFactor 1.5 at the same logical viewport.",
        "side-by-side-*.png: app on top, mockup board below. Mockup boards were rendered offline with their Google Fonts",
        "requests blocked, so they use the browser default sans-serif instead of IBM Plex / Noto; app uses Windows system fonts.",
        "The mockup control boards include a 40 px placeholder titlebar the real native window does not have.",
        "",
      ].join("\n"),
    );
  });
});
