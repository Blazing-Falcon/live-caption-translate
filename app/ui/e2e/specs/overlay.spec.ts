import { expect, test } from "@playwright/test";
import { NOW, calls, emitAll, emitEvent, emitNamed, open } from "./support";

test.describe("overlay (browser simulation with mocked transport)", () => {
  test("streams one utterance through pending, streaming and final in the subtitle bar", async ({ page }) => {
    const external = await open(page, "overlay");
    await emitEvent(page, NOW.asr(1, "大家好，欢迎来到我的直播间。"));
    await expect(page.locator('[data-state="pending"] .source')).toHaveText("大家好，欢迎来到我的直播间。");
    await expect(page.locator(".dots")).toBeVisible();
    await emitEvent(page, NOW.delta(1, "Hello everyone, welcome"));
    await expect(page.locator('[data-state="streaming"] .english')).toContainText("Hello everyone, welcome");
    await expect(page.locator(".caret")).toBeVisible();
    await emitEvent(page, NOW.final(1, "Hello everyone, welcome to my live stream."));
    await expect(page.locator(".caret")).toHaveCount(0);
    await expect(page.locator('[data-state="final"] .english')).toHaveText("Hello everyone, welcome to my live stream.");
    expect(external).toEqual([]);
  });

  test("shows only the two newest utterances in the bar and N in the panel", async ({ page }) => {
    await open(page, "overlay");
    for (let id = 1; id <= 4; id += 1) await emitAll(page, [NOW.asr(id, `句子${id}`), NOW.final(id, `Sentence ${id}`)]);
    await expect(page.locator("article")).toHaveCount(2);
    await expect(page.locator("article").first().locator(".english")).toHaveText("Sentence 3");

    const panel = await page.context().newPage();
    await open(panel, "overlay", { config: { overlay: { style: "panel", panel_lines: 4 } } });
    for (let id = 1; id <= 6; id += 1) await emitAll(panel, [NOW.asr(id, `句子${id}`), NOW.final(id, `Sentence ${id}`)]);
    await expect(panel.locator("article")).toHaveCount(4);
    await expect(panel.locator("article").last().locator(".english")).toHaveText("Sentence 6");
    await panel.close();
  });

  test("keeps the page transparent and the bar translucent with the configured opacity", async ({ page }) => {
    await open(page, "overlay", { config: { overlay: { background: 0.5, font_px: 30 } } });
    await emitAll(page, [NOW.asr(1, "你好"), NOW.final(1, "Hello")]);
    const body = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    const html = await page.evaluate(() => getComputedStyle(document.documentElement).backgroundColor);
    expect(body).toBe("rgba(0, 0, 0, 0)");
    expect(html).toBe("rgba(0, 0, 0, 0)");
    const bar = await page.locator(".bar").evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(bar).toBe("rgba(14, 15, 17, 0.5)");
    const size = await page.locator("article .english").evaluate((el) => getComputedStyle(el).fontSize);
    expect(size).toBe("30px");
  });

  test("fades older lines by age and reduces older text size", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page, "overlay");
    await emitAll(page, [NOW.asr(1, "一"), NOW.final(1, "One"), NOW.asr(2, "二"), NOW.final(2, "Two")]);
    const [older, newest] = await page.locator("article").evaluateAll((nodes) =>
      nodes.map((node) => ({ opacity: getComputedStyle(node).opacity, size: getComputedStyle(node.querySelector(".english") as Element).fontSize })),
    );
    expect(older).toEqual({ opacity: "0.55", size: "20.8px" });
    expect(newest).toEqual({ opacity: "1", size: "26px" });
  });

  test("renders every caption state with its worded label", async ({ page }) => {
    await open(page, "overlay", { config: { overlay: { style: "panel", panel_lines: 6 } } });
    await emitAll(page, [
      NOW.asr(1, "这件事说来话长。"),
      { type: "translation_failed", id: 1, reason: "server_unavailable", message: "" },
      NOW.asr(2, "然后我们再去下一个地图看看。"),
      { type: "skipped", id: 2, reason: "catch_up" },
      NOW.asr(3, "みんな、ありがとう！", "other", "ja"),
      NOW.asr(4, "OK, can you guys hear me now?", "english", "en"),
      NOW.asr(5, "你别误会，我不是那个意思。"),
      NOW.delta(5, "Don't get it wrong, that's"),
      NOW.asr(6, "你别误会。"),
    ]);
    const labels = await page.locator(".label").allInnerTexts();
    expect(labels.map((label) => label.toUpperCase())).toEqual([
      "TRANSLATION FAILED · TRANSLATOR IS RESTARTING",
      "SKIPPED TO CATCH UP",
      "JAPANESE · TURN ON IN SETTINGS TO TRANSLATE",
      "ENGLISH · NOT TRANSLATED",
    ]);
    await expect(page.locator('[data-state="streaming"] .caret')).toBeVisible();
    await expect(page.locator('[data-state="pending"] .dots')).toBeVisible();
  });

  test("replaces captions with worded status lines and shows the paused binding", async ({ page }) => {
    await open(page, "overlay", { config: { hotkeys: { pause: "Ctrl+Alt+P" } } });
    await emitAll(page, [NOW.asr(1, "你好"), NOW.final(1, "Hello")]);
    await emitEvent(page, { type: "listening_state", state: "paused" });
    await expect(page.locator(".status")).toHaveText("Paused · Ctrl Alt P to resume");
    await expect(page.locator("article")).toHaveCount(0);
    await emitEvent(page, { type: "listening_state", state: "listening" });
    await emitEvent(page, { type: "source_state", state: "apps_not_running", detail: "Chrome, VLC" });
    await expect(page.locator(".status")).toHaveText("Waiting for Chrome, VLC");
    await emitEvent(page, { type: "source_state", state: "playing", detail: null });
    await expect(page.locator("article .english")).toHaveText("Hello");
  });

  test("fades and clears lines after the quiet period, sparing in-flight translations", async ({ page }) => {
    await page.clock.install({ time: 0 });
    await open(page, "overlay", { config: { overlay: { expire_s: 8 } } });
    await emitAll(page, [NOW.asr(1, "一"), NOW.final(1, "One"), NOW.asr(2, "二"), NOW.delta(2, "Tw")]);
    await expect(page.locator("article")).toHaveCount(2);
    await page.clock.runFor(8100);
    await expect(page.locator('article[data-id="1"]')).toHaveClass(/fading/);
    await page.clock.runFor(300);
    await expect(page.locator('article[data-id="1"]')).toHaveCount(0);
    await expect(page.locator('article[data-id="2"] .english')).toContainText("Tw");
    await emitEvent(page, NOW.final(2, "Two"));
    await page.clock.runFor(100);
    await page.clock.runFor(300);
    await expect(page.locator("article")).toHaveCount(0);
  });

  test("a new asr_final cancels a fade in progress", async ({ page }) => {
    await page.clock.install({ time: 0 });
    await open(page, "overlay");
    await emitAll(page, [NOW.asr(1, "一"), NOW.final(1, "One")]);
    await page.clock.runFor(8050);
    await expect(page.locator('article[data-id="1"]')).toHaveClass(/fading/);
    await emitEvent(page, NOW.asr(2, "二"));
    await page.clock.runFor(500);
    await expect(page.locator("article")).toHaveCount(2);
    await expect(page.locator('article[data-id="1"]')).not.toHaveClass(/fading/);
  });

  test("shows the listening chip for three seconds", async ({ page }) => {
    await page.clock.install({ time: 0 });
    await open(page, "overlay", { state: { listening: "starting" } });
    await emitEvent(page, { type: "source_changed", info: { mode: "system", label: "Speakers (Realtek)", sample_rate: 48000, channels: 2 } });
    await emitEvent(page, { type: "listening_state", state: "listening" });
    await expect(page.getByTestId("status-chip")).toContainText("Listening · Speakers (Realtek)");
    await page.clock.runFor(2900);
    await expect(page.getByTestId("status-chip")).toBeVisible();
    await page.clock.runFor(200);
    await expect(page.getByTestId("status-chip")).toHaveCount(0);
  });

  test("honours reduced motion: no animations, immediate expiry", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.clock.install({ time: 0 });
    await open(page, "overlay");
    await emitAll(page, [NOW.asr(1, "一"), NOW.delta(1, "On")]);
    const animation = await page.locator("article").evaluate((el) => getComputedStyle(el).animationName);
    const caret = await page.locator(".caret").evaluate((el) => getComputedStyle(el).animationName);
    expect(animation).toBe("none");
    expect(caret).toBe("none");
    await emitEvent(page, NOW.final(1, "One"));
    await page.clock.runFor(8010);
    await expect(page.locator("article")).toHaveCount(0);
  });

  test("move mode shows the dashed frame and Lock calls set_overlay_moving(false)", async ({ page }) => {
    await open(page, "overlay", { state: { overlay_moving: true } });
    const frame = page.getByTestId("move-frame");
    await expect(frame).toBeVisible();
    expect(await frame.evaluate((el) => getComputedStyle(el).borderTopStyle)).toBe("dashed");
    await expect(page.getByText("Move or resize, then lock")).toBeVisible();
    await expect(page.locator("kbd")).toHaveText("Ctrl Shift L");
    await page.getByRole("button", { name: "Lock" }).click();
    await expect.poll(() => calls(page, "set_overlay_moving")).toEqual([{ cmd: "set_overlay_moving", args: { moving: false } }]);
    await emitNamed(page, "overlay://mode", { moving: false });
    await expect(frame).toHaveCount(0);
  });

  test("panel header appears on hover and its buttons call pause and the control window", async ({ page }) => {
    await open(page, "overlay", { config: { overlay: { style: "panel" } } });
    const header = page.getByTestId("panel-header");
    await expect(header).toBeHidden();
    await page.getByTestId("caption-panel").hover();
    await expect(header).toBeVisible();
    await expect(header).toContainText("Listening · Speakers (Realtek)");
    await page.getByRole("button", { name: "Pause" }).click();
    await page.getByRole("button", { name: "Open settings" }).click();
    await expect.poll(async () => (await calls(page, "pause_listening")).length).toBe(1);
    await expect.poll(async () => (await calls(page, "show_control_window")).length).toBe(1);
  });

  test("the optional overlay://hover event reveals the panel header without a pointer", async ({ page }) => {
    await open(page, "overlay", { config: { overlay: { style: "panel" } } });
    await expect(page.getByTestId("panel-header")).toBeHidden();
    await emitNamed(page, "overlay://hover", { hover: true });
    await expect(page.getByTestId("panel-header")).toBeVisible();
  });
});
