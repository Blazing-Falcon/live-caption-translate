import { expect, test, type Page } from "@playwright/test";
import { NOW, emitEvent, open } from "./support";
import type { PipelineEvent } from "../../src/lib/types";

test.describe("overlay (browser simulation with mocked transport)", () => {

  test("shows a live line, then pending, streaming and the final in the same elements", async ({ page }) => {
    const external = await open(page, "overlay", { config: { overlay: { style: "panel" } } });
    await emitEvent(page, NOW.partial(1, "这次华为发布的呢是整个"));
    await expect(page.locator('[data-state="open"] .source')).toHaveText("这次华为发布的呢是整个");
    await expect(page.locator(".english")).toHaveCount(0);
    await expect(page.locator(".dots")).toHaveCount(0);
    await page.locator("article").evaluate((el) => {
      (el as HTMLElement & { __tag?: string }).__tag = "article";
    });

    await emitEvent(page, NOW.asr(1, "这次华为发布的呢是整个mate90家族啊，", "chinese", "commit"));
    await expect(page.locator('article[data-state="pending"] .dots')).toHaveCount(1);

    await emitEvent(page, NOW.delta(1, "This time, Huawei released the whole"));
    const english = page.locator("article .english");
    await expect(english).toHaveText("This time, Huawei released the whole");
    await expect(page.locator('article[data-state="streaming"] .english .caret')).toHaveCount(1);
    await expect(page.locator(".dots")).toHaveCount(0);
    await english.evaluate((el) => {
      (el as HTMLElement & { __tag?: string }).__tag = "english";
    });
    const panelHeight = (await page.locator(".panel").boundingBox())?.height;

    await emitEvent(page, NOW.final(1, "This time, Huawei released the whole Mate 90 family."));
    await expect(english).toHaveText("This time, Huawei released the whole Mate 90 family.");
    const after = await page.locator("article").evaluate((el) => ({
      article: (el as HTMLElement & { __tag?: string }).__tag,
      english: (el.querySelector(".english") as HTMLElement & { __tag?: string }).__tag,
      animations: el.getAnimations().filter((animation) => animation instanceof CSSAnimation).length,
      caret: !!el.querySelector(".caret"),
      count: document.querySelectorAll("article").length,
    }));
    expect(after).toEqual({ article: "article", english: "english", animations: 0, caret: false, count: 1 });
    expect(await english.evaluate((el) => getComputedStyle(el).color)).toBe("rgb(244, 244, 242)");
    expect((await page.locator(".panel").boundingBox())?.height).toBe(panelHeight);
    expect(external).toEqual([]);
  });

});

interface Box {
  key: number;
  top: number;
  bottom: number;
}

const SETTLE_MS = 320;

async function settle(page: Page): Promise<void> {
  await page.waitForTimeout(SETTLE_MS);
}

/** Every block in `scope` that crosses the viewport's top and bottom edges. */
async function visibleBoxes(page: Page, scope: string): Promise<Box[]> {
  return page.evaluate((selector) => {
    const viewport = document.querySelector<HTMLElement>(selector);
    if (!viewport) return [];
    const box = viewport.getBoundingClientRect();
    return [...viewport.querySelectorAll<HTMLElement>("[data-block]")]
      .map((element) => ({
        key: Number(element.dataset.block),
        top: element.getBoundingClientRect().top,
        bottom: element.getBoundingClientRect().bottom,
      }))
      .filter((entry) => entry.bottom > box.top + 0.5 && entry.top < box.bottom - 0.5);
  }, scope);
}

function scopeOf(style: "panel" | "flow"): string {
  return style === "panel" ? '[data-testid="caption-panel"] .viewport' : '[data-testid="flow-text"] .para.english .viewport';
}

/** No block may move down, and every block that moves up must move by the same amount: one
 * scroll step for the whole column, never two. */
function expectStable(before: Box[], after: Box[], label: string): void {
  const now = new Map(after.map((entry) => [entry.key, entry.top]));
  const moves: number[] = [];
  for (const entry of before) {
    const top = now.get(entry.key);
    if (top === undefined) continue;
    const move = top - entry.top;
    expect(move, `${label}: block ${entry.key} moved down by ${move.toFixed(1)}px`).toBeLessThanOrEqual(0.5);
    if (move < -0.5) moves.push(move);
  }
  for (const move of moves) {
    expect(Math.abs(move - (moves[0] as number)), `${label}: block moved ${move.toFixed(1)}px, not one scroll step`).toBeLessThanOrEqual(0.5);
  }
}

/** Three units with growing and shrinking previews, a seal, a streaming final, a second-pass
 * correction and a status that appears. */
const SEQUENCE: [string, PipelineEvent][] = [
  ["line 1 heard", NOW.partial(1, "这次华为发布的")],
  ["line 1 final", NOW.asr(1, "这次华为发布的呢是整个", "chinese", "commit")],
  ["preview 1 short", NOW.preview(1, [1], "This time Huawei")],
  ["preview 1 long", NOW.preview(1, [1], "This time Huawei released the whole Mate 90 family and more phones")],
  ["preview 1 short again", NOW.preview(1, [1], "This time Huawei")],
  ["line 2 heard", NOW.partial(2, "还有一个")],
  ["line 2 final", NOW.asr(2, "还有一个新的手表，", "chinese", "commit")],
  ["line 3 heard", NOW.partial(3, "我们")],
  ["preview 2 and 3", NOW.preview(2, [2, 3], "There is also a new watch, and we")],
  ["line 3 final", NOW.asr(3, "我们去看看。")],
  ["unit 1 sealed", NOW.sealed(1, [1], "这次华为发布的呢是整个")],
  ["unit 2 sealed", NOW.sealed(2, [2, 3], "还有一个新的手表，我们去看看。")],
  ["unit 1 streams", NOW.delta(1, "This time, Huawei released the whole")],
  ["unit 1 final", NOW.final(1, "This time, Huawei released the whole Mate 90 family.")],
  ["unit 2 streams", NOW.delta(2, "There is also a new watch, and")],
  ["second pass corrects line 4", { ...NOW.asr(4, "我们去看看。"), second_pass: { outcome: "used", live_text: "我们去看看", window_start_ms: 0, window_end_ms: 1000, distance: 1, decode_ms: 50, refine_ms: 60 } } as PipelineEvent],
  ["unit 2 final", NOW.final(2, "There is also a new watch, and let's go take a look.")],
  ["line 5 heard", NOW.partial(5, "还有一件事")],
  ["line 5 final", NOW.asr(5, "还有一件事。")],
  ["line 6 heard", NOW.partial(6, "我想说")],
  ["line 7 heard", NOW.partial(7, "谢谢大家")],
  ["line 7 final", NOW.asr(7, "谢谢大家。")],
  ["unit 5 sealed", NOW.sealed(5, [5], "还有一件事。")],
  ["unit 5 final", NOW.final(5, "There is one more thing.")],
  ["status appears", { type: "engine_status", engine: "translator", state: "failed", message: null }],
];

for (const style of ["panel", "flow"] as const) {
  for (const lines of [1, 10]) {
    test(`keeps every visible block still or moves it up in one step (${style}, ${style}_lines ${lines})`, async ({ page }) => {
      const overlay = style === "panel" ? { style, panel_lines: lines } : { style, flow_lines: lines };
      await open(page, "overlay", { config: { overlay } });
      const scope = scopeOf(style);

      let before = await visibleBoxes(page, scope);
      expect(before.length, "nothing visible before the first event").toBe(0);

      for (const [label, event] of SEQUENCE) {
        await emitEvent(page, event);
        await settle(page);
        const after = await visibleBoxes(page, scope);
        expectStable(before, after, label);
        before = after;
      }

      expect(before.length, "some captions are visible at the end").toBeGreaterThan(0);
      await expect(page.locator(".status")).toHaveCount(1);
      if (style === "panel") {
        // Block snap: the top edge falls on a block boundary, so it never cuts a block in half.
        const viewportBox = await page.locator(scope).boundingBox();
        for (const entry of before) {
          expect(entry.top, `block ${entry.key} starts above the top edge`).toBeGreaterThanOrEqual((viewportBox?.y ?? 0) - 0.5);
        }
      }
    });
  }
}
