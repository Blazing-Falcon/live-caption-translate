import { expect, test } from "@playwright/test";
import { NOW, calls, emitAll, emitEvent, emitNamed, open } from "./support";

test.describe("overlay (browser simulation with mocked transport)", () => {

  test("shows a live line, then a dimmed draft, then the final replacing it in the same elements", async ({ page }) => {
    const external = await open(page, "overlay");
    await emitEvent(page, NOW.partial(1, "这次华为发布的呢是整个"));
    await expect(page.locator('[data-state="live"] .source')).toHaveText("这次华为发布的呢是整个");
    await expect(page.locator(".english")).toHaveCount(0);
    await expect(page.locator(".dots")).toHaveCount(0);
    await emitEvent(page, NOW.draft(1, "This time, Huawei released the whole family"));
    const english = page.locator("article .english");
    await expect(english).toHaveText("This time, Huawei released the");
    const style = await english.evaluate((el) => {
      const css = getComputedStyle(el);
      return {
        color: css.color, fontStyle: css.fontStyle, filter: css.filter, animation: css.animationName,
        decoration: css.textDecorationLine, opacity: css.opacity, size: css.fontSize, weight: css.fontWeight, caret: !!el.querySelector(".caret"),
      };
    });
    expect(style).toEqual({
      color: "rgb(180, 183, 188)", fontStyle: "normal", filter: "none", animation: "none", decoration: "none", opacity: "1",
      size: "26px", weight: "500", caret: false,
    });

    await emitEvent(page, NOW.asr(1, "这次华为发布的呢是整个mate90家族啊，", "chinese", "zh", "commit"));
    await expect(english).toHaveText("This time, Huawei released the whole family");
    await expect(page.locator('article[data-state="pending"]')).toHaveCount(1);
    await expect(page.locator(".dots")).toHaveCount(0);
    await page.waitForTimeout(250);
    await page.locator("article").evaluate((el) => {
      (el as HTMLElement & { __tag?: string }).__tag = "article";
      (el.querySelector(".english") as HTMLElement & { __tag?: string }).__tag = "english";
    });
    const barHeight = (await page.locator(".bar").boundingBox())?.height;

    await emitEvent(page, NOW.final(1, "This time, Huawei released the whole Mate 90 family."));
    await expect(english).toHaveText("This time, Huawei released the whole Mate 90 family.");
    const after = await page.locator("article").evaluate((el) => ({
      article: (el as HTMLElement & { __tag?: string }).__tag,
      english: (el.querySelector(".english") as HTMLElement & { __tag?: string }).__tag,
      slideUp: el.getAnimations().filter((animation) => animation instanceof CSSAnimation).length,
      count: document.querySelectorAll("article").length,
    }));
    expect(after).toEqual({ article: "article", english: "english", slideUp: 0, count: 1 });
    await page.waitForTimeout(250);
    expect(await english.evaluate((el) => getComputedStyle(el).color)).toBe("rgb(244, 244, 242)");
    expect((await page.locator(".bar").boundingBox())?.height).toBe(barHeight);
    expect(external).toEqual([]);
  });

});
