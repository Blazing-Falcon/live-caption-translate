import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { test, type Page } from "@playwright/test";
import type { PipelineEvent } from "../../src/lib/types";
import { NOW, emitAll, emitEvent, open, placeBar, placePanel } from "./support";

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
        "",
      ].join("\n"),
    );
  });
});
