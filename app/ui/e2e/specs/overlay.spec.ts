import { expect, test, type Page } from "@playwright/test";
import { NOW, emitEvent, open } from "./support";
import type { PipelineEvent } from "../../src/lib/types";

test.describe("overlay (browser simulation with mocked transport)", () => {

  test("film subtitles show only the newest block and the one before it, dimmed", async ({ page }) => {
    await open(page, "overlay", { config: { overlay: { style: "subtitles" } } });
    for (const [id, zh, en] of [[1, "你好。", "Hello."], [2, "我们走吧。", "Let's go."], [3, "他们来了。", "They're here."]] as const) {
      await emitEvent(page, NOW.asr(id, zh));
      await emitEvent(page, NOW.sealed(id, [id], zh));
      await emitEvent(page, NOW.final(id, en));
    }
    const shown = page.getByTestId("subtitles").locator("[data-block]");
    await expect(shown).toHaveCount(2);
    await expect(shown.locator(".english")).toHaveText(["Let's go.", "They're here."]);
    await expect(shown.first()).toHaveClass(/older/);
    await expect(shown.last()).not.toHaveClass(/older/);
  });

  test("shows a live line, then pending, streaming and the final in the same elements", async ({ page }) => {
    const external = await open(page, "overlay", { config: { overlay: { style: "panel", english_color: "#ffd84d" } } });
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
    expect(await english.evaluate((el) => getComputedStyle(el).color)).toBe("rgb(255, 216, 77)");
    expect((await page.locator(".panel").boundingBox())?.height).toBe(panelHeight);
    expect(external).toEqual([]);
  });

});

interface Box {
  key: number;
  top: number;
  bottom: number;
  /** The viewport's own top, so a paragraph pushed down by the one above it is not counted as
   * text sliding inside the viewport. */
  frame: number;
  /** The block's own text, so a re-wrap caused by that text changing is not counted as a slide. */
  text: string;
}

/** A window the size of the app's own panel, small enough that the sequence overflows it. */
const SIZE = { width: 560, height: 360 };
const SETTLE_MS = 320;
/** Samples inside one settle window, so motion that starts and ends between two events is seen. */
const SAMPLES = 8;

/** Every block in `scope`, with its position relative to the viewport's top edge. */
async function boxes(page: Page, scope: string): Promise<Box[]> {
  return page.evaluate((selector) => {
    const viewport = document.querySelector<HTMLElement>(selector);
    if (!viewport) return [];
    const box = viewport.getBoundingClientRect();
    return [...viewport.querySelectorAll<HTMLElement>("[data-block]")].map((element) => {
      const rect = element.getBoundingClientRect();
      return {
        key: Number(element.dataset.block),
        top: rect.top - box.top,
        bottom: rect.bottom - box.top,
        frame: box.top,
        text: element.textContent ?? "",
      };
    });
  }, scope);
}

async function viewportHeight(page: Page, scope: string): Promise<number> {
  return page.evaluate((selector) => document.querySelector<HTMLElement>(selector)?.clientHeight ?? 0, scope);
}

function scopeOf(style: "panel" | "flow"): string {
  return style === "panel" ? '[data-testid="caption-panel"] .viewport' : '[data-testid="flow-text"] .para.english .viewport';
}

/**
 * The owner's rule: text a reader is already reading never moves down. A block may grow (a final
 * replacing a shorter preview, a longer line wrapping), and a block below a growing one moves down
 * by exactly that growth; anything more is the viewport letting the text slide. Everything that
 * moves up must move by one amount: one scroll step for the column.
 *
 * In running text the blocks are spans in one paragraph, so a block whose own text changed can
 * re-wrap onto the next line and move down by itself. That is the text being replaced, not the
 * column sliding, and it still pushes the blocks after it down by its own growth.
 */
function expectStable(before: Box[], after: Box[], label: string, allowReflow: boolean): void {
  const now = new Map(after.map((entry) => [entry.key, entry]));
  const moves: number[] = [];
  let pushed = 0;
  for (const entry of before) {
    const next = now.get(entry.key);
    if (next === undefined) continue;
    // Running text puts the English paragraph under the Chinese one, so a Chinese paragraph that
    // appears moves the whole English paragraph down. Inside the viewport nothing moved; the box
    // itself did, and the difference is what this rule is about.
    const frame = next.frame - entry.frame;
    const move = next.top - entry.top - frame;
    const reflowed = next.text !== entry.text;
    if (!(allowReflow && reflowed)) {
      expect(
        move - pushed,
        `${label}: block ${entry.key} moved down by ${move.toFixed(1)}px (${pushed.toFixed(1)}px of that is text above it growing)`,
      ).toBeLessThanOrEqual(0.5);
      if (move < -0.5) moves.push(move);
    }
    pushed += Math.max(0, next.bottom - entry.bottom - frame);
  }
  for (const move of moves) {
    expect(
      Math.abs(move - (moves[0] as number)),
      `${label}: block moved ${move.toFixed(1)}px, not one scroll step`,
    ).toBeLessThanOrEqual(0.5);
  }
}

/** Lines are heard, sealed and translated one at a time, with a preview on the open line that
 * grows and shrinks, and a second-pass correction of line 3 before it is sealed. Eight units
 * overflow the window, so the column really scrolls and prunes. */
const SEQUENCE: [string, PipelineEvent][] = [
  ["line 1 heard", NOW.partial(1, "这次华为发布的")],
  ["line 1 final", NOW.asr(1, "这次华为发布的呢是整个", "chinese", "commit")],
  ["preview short", NOW.preview(1, [1], "This time Huawei")],
  ["preview long", NOW.preview(1, [1], "This time Huawei released the whole Mate 90 family and more phones")],
  ["preview short again", NOW.preview(1, [1], "This time Huawei")],
  ["unit 1 sealed", NOW.sealed(1, [1], "这次华为发布的呢是整个")],
  ["unit 1 streams", NOW.delta(1, "This time, Huawei released the whole")],
  ["unit 1 final", NOW.final(1, "This time, Huawei released the whole Mate 90 family.")],
  ["line 2 heard", NOW.partial(2, "还有一个")],
  ["line 2 final", NOW.asr(2, "还有一个新的手表，", "chinese", "commit")],
  ["unit 2 sealed", NOW.sealed(2, [2], "还有一个新的手表，")],
  ["unit 2 final", NOW.final(2, "There is also a new watch.")],
  ["line 3 heard", NOW.partial(3, "我们去看看")],
  ["line 3 final", NOW.asr(3, "我们去看看。")],
  ["line 3 corrected", { ...NOW.asr(3, "我们去看看吧。"), second_pass: { outcome: "used", live_text: "我们去看看", window_start_ms: 0, window_end_ms: 1000, distance: 1, decode_ms: 50, refine_ms: 60 } } as PipelineEvent],
  ["unit 3 sealed", NOW.sealed(3, [3], "我们去看看吧。")],
  ["unit 3 final", NOW.final(3, "Let's go take a look.")],
  ["line 4 heard", NOW.partial(4, "还有一件事")],
  ["line 4 final", NOW.asr(4, "还有一件事。")],
  ["unit 4 sealed", NOW.sealed(4, [4], "还有一件事。")],
  ["unit 4 final", NOW.final(4, "There is one more thing.")],
  ["line 5 heard", NOW.partial(5, "我想说")],
  ["preview 5 long", NOW.preview(5, [5], "I want to say something about the new phone and its camera")],
  ["preview 5 short", NOW.preview(5, [5], "I want to say")],
  ["line 5 final", NOW.asr(5, "我想说一句话。")],
  ["unit 5 sealed", NOW.sealed(5, [5], "我想说一句话。")],
  ["unit 5 final", NOW.final(5, "I want to say something.")],
  ["line 6 heard", NOW.partial(6, "谢谢大家")],
  ["line 6 final", NOW.asr(6, "谢谢大家。")],
  ["unit 6 sealed", NOW.sealed(6, [6], "谢谢大家。")],
  ["unit 6 final", NOW.final(6, "Thank you all.")],
  ["line 7 heard", NOW.partial(7, "就这样吧")],
  ["preview 7 long", NOW.preview(7, [7], "That is everything for today, so let us stop here")],
  ["preview 7 short", NOW.preview(7, [7], "That is everything")],
  ["line 7 final", NOW.asr(7, "就这样吧。")],
  ["unit 7 sealed", NOW.sealed(7, [7], "就这样吧。")],
  ["unit 7 final", NOW.final(7, "That's it.")],
  // The column overflows by now, so a preview that shrinks would pull the offset down.
  ["line 8 heard", NOW.partial(8, "最后再说一句")],
  ["preview 8 long", NOW.preview(8, [8], "One last thing that I want to say before we finish")],
  ["preview 8 short", NOW.preview(8, [8], "One last thing")],
  ["status appears", { type: "engine_status", engine: "translator", state: "failed", message: null }],
];

for (const style of ["panel", "flow"] as const) {
  for (const lines of [1, 10]) {
    test(`keeps every block still or moves it up in one step (${style}, ${style}_lines ${lines})`, async ({ page }) => {
      await page.setViewportSize(SIZE);
      const overlay = style === "panel" ? { style, panel_lines: lines } : { style, flow_lines: lines };
      await open(page, "overlay", { config: { overlay } });
      const scope = scopeOf(style);
      const height = await viewportHeight(page, scope);
      expect(height, "the viewport has a real height").toBeGreaterThan(0);

      let before = await boxes(page, scope);
      expect(before.length, "nothing visible before the first event").toBe(0);
      let scrolled = false;

      for (const [label, event] of SEQUENCE) {
        await emitEvent(page, event);
        for (let sample = 0; sample < SAMPLES; sample += 1) {
          await page.waitForTimeout(SETTLE_MS / SAMPLES);
          const after = await boxes(page, scope);
          expectStable(before, after, label, style === "flow");
          if (after.some((entry) => entry.top < -0.5)) scrolled = true;
          before = after;
        }
      }

      // The test is only worth its runtime if the column actually overflowed and scrolled.
      expect(before.length, "some captions are visible at the end").toBeGreaterThan(0);
      expect(scrolled, "the column never scrolled, so the test proved nothing").toBe(true);
      await expect(page.locator(".status")).toHaveCount(1);
    });
  }
}
