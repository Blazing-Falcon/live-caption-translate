import { expect, test } from "@playwright/test";
import { NOW, calls, emitAll, emitEvent, emitNamed, open } from "./support";

test.describe("overlay (browser simulation with mocked transport)", () => {

  test("shows a live line, then pending, streaming and the final in the same elements", async ({ page }) => {
    const external = await open(page, "overlay");
    await emitEvent(page, NOW.partial(1, "这次华为发布的呢是整个"));
    await expect(page.locator('[data-state="live"] .source')).toHaveText("这次华为发布的呢是整个");
    await expect(page.locator(".english")).toHaveCount(0);
    await expect(page.locator(".dots")).toHaveCount(0);
    await page.waitForTimeout(250);
    await page.locator("article").evaluate((el) => {
      (el as HTMLElement & { __tag?: string }).__tag = "article";
    });

    await emitEvent(page, NOW.asr(1, "这次华为发布的呢是整个mate90家族啊，", "chinese", "zh", "commit"));
    await expect(page.locator('article[data-state="pending"] .dots')).toHaveCount(1);

    await emitEvent(page, NOW.delta(1, "This time, Huawei released the whole"));
    const english = page.locator("article .english");
    await expect(english).toHaveText("This time, Huawei released the whole");
    await expect(page.locator('article[data-state="streaming"] .english .caret')).toHaveCount(1);
    await expect(page.locator(".dots")).toHaveCount(0);
    await english.evaluate((el) => {
      (el as HTMLElement & { __tag?: string }).__tag = "english";
    });
    const barHeight = (await page.locator(".bar").boundingBox())?.height;

    await emitEvent(page, NOW.final(1, "This time, Huawei released the whole Mate 90 family."));
    await expect(english).toHaveText("This time, Huawei released the whole Mate 90 family.");
    const after = await page.locator("article").evaluate((el) => ({
      article: (el as HTMLElement & { __tag?: string }).__tag,
      english: (el.querySelector(".english") as HTMLElement & { __tag?: string }).__tag,
      slideUp: el.getAnimations().filter((animation) => animation instanceof CSSAnimation).length,
      caret: !!el.querySelector(".caret"),
      count: document.querySelectorAll("article").length,
    }));
    expect(after).toEqual({ article: "article", english: "english", slideUp: 0, caret: false, count: 1 });
    expect(await english.evaluate((el) => getComputedStyle(el).color)).toBe("rgb(244, 244, 242)");
    expect((await page.locator(".bar").boundingBox())?.height).toBe(barHeight);
    expect(external).toEqual([]);
  });

});
