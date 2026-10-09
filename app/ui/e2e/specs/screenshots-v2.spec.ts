import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { test, type Page } from "@playwright/test";
import type { PipelineEvent } from "../../src/lib/types";
import { DRAFT, NOW, READY, STATS_V2, emitAll, emitEvent, open, placeBar, placePanel } from "./support";

/**
 * Look for the owner: the bar and the panel with a live line, drafts and finals.
 * Same browser simulation as screenshots.spec.ts: headless Chromium, mocked transport, synthetic text, Windows
 * system fonts if present. Not a native Windows run.
 */
const OUT = fileURLToPath(new URL("../../test-results/screenshots-v2/", import.meta.url));
mkdirSync(OUT, { recursive: true });

const SIZE = { width: 1280, height: 720 };

const PREVIOUS: PipelineEvent[] = [
  NOW.asr(1, "这波操作真的太秀了，我直接看傻了。"),
  NOW.final(1, "This move is really amazing; I'm completely amazed."),
];

const SCENES: { name: string; title: string; events: PipelineEvent[] }[] = [
  {
    name: "live-chinese-only",
    title: "live line, Chinese only (the first draft follows within about a second)",
    events: [NOW.partial(2, "这次华为发布的呢是整个")],
  },
  {
    name: "live-with-draft",
    title: "live line with a dimmed draft (the newest two words are held back)",
    events: [
      NOW.partial(2, "这次华为发布的呢是整个mate90家族"),
      NOW.draft(2, "This time, Huawei released the whole Mate 90 family", 3),
    ],
  },
  {
    name: "pending-full-draft",
    title: "committed clause: the whole draft, waiting for the final",
    events: [
      NOW.partial(2, "这次华为发布的呢是整个mate90家族"),
      NOW.draft(2, "This time, Huawei released the whole Mate 90 family", 3),
      NOW.asr(2, "这次华为发布的呢是整个mate90家族啊，", "chinese", "zh", "commit"),
    ],
  },
  {
    name: "final-under-live",
    title: "a final line above a new live line with a draft",
    events: [
      NOW.partial(2, "这次华为发布的呢是整个mate90家族"),
      NOW.draft(2, "This time, Huawei released the whole Mate 90 family", 3),
    ],
  },
  {
    name: "skipped-with-draft",
    title: "skipped final: the last draft stays, dimmed, with no label",
    events: [
      NOW.partial(2, "然后我们再去下一个地图看看"),
      NOW.draft(2, "Then we will go to the next map and see", 2),
      NOW.asr(2, "然后我们再去下一个地图看看。"),
      NOW.skipped(2),
    ],
  },
];

async function scene(page: Page, style: "bar" | "panel", events: PipelineEvent[], withPrevious: boolean): Promise<void> {
  await page.clock.install({ time: 0 });
  await open(page, "overlay", style === "panel" ? { config: { overlay: { style: "panel", background: 0.86 } } } : {});
  if (style === "panel") await placePanel(page);
  else await placeBar(page);
  await emitEvent(page, { type: "source_changed", info: { mode: "system", label: "system audio", sample_rate: 48000, channels: 2 } });
  await emitAll(page, [...(withPrevious ? PREVIOUS : []), ...events]);
  await page.clock.pauseAt(1000);
}

test.describe("v2 screenshots (browser simulation)", () => {
  for (const style of ["bar", "panel"] as const) {
    for (const item of SCENES) {
      test(`${style}: ${item.name}`, async ({ browser }) => {
        const ctx = await browser.newContext({ viewport: SIZE, deviceScaleFactor: 1, reducedMotion: "reduce", colorScheme: "light", locale: "en-US" });
        const page = await ctx.newPage();
        await scene(page, style, item.events, item.name === "final-under-live" || style === "panel");
        await page.waitForSelector("article");
        await page.screenshot({ path: join(OUT, `${style}-${item.name}.png`), animations: "disabled" });
        await ctx.close();
      });
    }
  }

  const CONTROL = { width: 900, height: 780 };
  const CONTROL_SHOTS = [
    ["performance-light", "light", "Performance page, Continuous, light theme"],
    ["performance-dark", "dark", "Performance page, Continuous, dark theme"],
    ["performance-draft-unavailable-light", "light", "Performance page, Light because the draft model is not downloaded"],
    ["overlay-page-light", "light", "Overlay page with Show Chinese while someone is speaking and Draft text"],
    ["first-run-optional-light", "light", "First run on a smaller PC: the draft model is optional"],
  ] as const;

  for (const [name, scheme] of CONTROL_SHOTS) {
    test(`control window: ${name}`, async ({ browser }) => {
      const ctx = await browser.newContext({ viewport: CONTROL, deviceScaleFactor: 1, reducedMotion: "reduce", colorScheme: scheme, locale: "en-US" });
      const page = await ctx.newPage();
      if (name === "first-run-optional-light") {
        const missing = READY.map((model, index) => (index === 0 ? model : { ...model, bytes_done: 0, state: "missing" as const }));
        await open(page, "control", { models: [...missing, DRAFT], state: { models_ready: false, listening: "paused" } });
        await page.waitForSelector(".model");
      } else if (name === "overlay-page-light") {
        await open(page, "control", { config: { capture: { mode: "system" } }, stats: STATS_V2 });
        await page.getByRole("tab", { name: "Overlay" }).click();
        await page.waitForSelector("#draft-display-help");
      } else {
        const unavailable = name === "performance-draft-unavailable-light";
        await open(page, "control", {
          config: { capture: { mode: "system" } },
          stats: unavailable ? { ...STATS_V2, mode: "light", mode_reason: "draft_unavailable", cpu_draft_pct: 0, rss_draft_mb: 0, word_first_p50_ms: null } : STATS_V2,
          models: unavailable ? [...READY, DRAFT] : [...READY, { ...DRAFT, state: "ready", bytes_done: DRAFT.bytes_total }],
        });
        await page.getByRole("tab", { name: "Performance" }).click();
        await page.waitForSelector('[data-testid="mode-status"]');
      }
      await page.screenshot({ path: join(OUT, `${name}.png`), animations: "disabled" });
      await ctx.close();
    });
  }

  test("writes an index of the v2 screenshots", async () => {
    writeFileSync(
      join(OUT, "README.txt"),
      [
        "Captions: live line, dimmed drafts, finals.",
        "Browser simulation (headless Chromium via Playwright, mocked Tauri transport, synthetic text, 100% scale, reduced motion).",
        "It is NOT a native Windows run: WebView2 rendering, real Windows DPI and the look over real video are untested here.",
        "Regenerate: npm --prefix app/ui run e2e -- screenshots-v2.spec.ts",
        "",
        ...["bar", "panel"].flatMap((style) => SCENES.map((item) => `${style}-${item.name}.png: ${item.title}`)),
        ...CONTROL_SHOTS.map(([name, , title]) => `${name}.png: ${title}`),
        "",
      ].join("\n"),
    );
  });
});
