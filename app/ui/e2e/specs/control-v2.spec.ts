import { expect, test, type Page } from "@playwright/test";
import type { PipelineStats } from "../../src/lib/types";
import { DRAFT, READY, STATS_V2, calls, emitNamed, open } from "./support";

const expectPatch = (page: Page, patch: unknown): Promise<void> =>
  expect.poll(async () => (await calls(page, "set_config")).at(-1)?.args.patch).toEqual(patch);

const sendStats = (page: Page, patch: Partial<PipelineStats>): Promise<void> =>
  emitNamed(page, "pipeline://event", { type: "stats", ...STATS_V2, ...patch });

test.describe("control window v2 (browser simulation with mocked transport)", () => {
  test("performance page: caption speed select, help text and status line", async ({ page }) => {
    const external = await open(page, "control", { stats: STATS_V2 });
    await page.getByRole("tab", { name: "Performance" }).click();
    const select = page.getByRole("combobox", { name: "Caption speed" });
    await expect(select.locator("option")).toHaveText([
      "Automatic (recommended)",
      "Continuous",
      "Light",
      "Wait for full sentences",
    ]);
    await expect(select).toHaveValue("auto");
    await expect(page.getByTestId("caption-speed-help")).toHaveText("Uses Continuous on PCs with 6 or more cores, Light on smaller PCs.");
    await expect(page.getByTestId("mode-status")).toHaveText("Now: Continuous");
    const first = await page.locator("section.page > *").nth(1).locator("select").count();
    expect(first).toBe(1);

    await select.selectOption("off");
    await expectPatch(page, { latency: { mode: "off" } });
    await expect(page.getByTestId("caption-speed-help")).toHaveText("Translates after the speaker pauses, like version 1.");
    await select.selectOption("light");
    await expect(page.getByTestId("caption-speed-help")).toHaveText("Translates each phrase as soon as it ends. Less CPU, no draft text.");
    await select.selectOption("continuous");
    await expectPatch(page, { latency: { mode: "continuous" } });
    await expect(page.getByTestId("caption-speed-help")).toHaveText("English follows the speaker within about a second. Uses the most CPU.");

    const statuses: [Partial<PipelineStats>, string][] = [
      [{ mode: "light", mode_reason: "cpu" }, "Now: Light, because the PC is busy"],
      [{ mode: "light", mode_reason: "lag" }, "Now: Light, because translation fell behind"],
      [{ mode: "light", mode_reason: "draft_unavailable" }, "Now: Light, because the draft model is not downloaded"],
      [{ mode: "light", mode_reason: "user" }, "Now: Light"],
      [{ mode: "off", mode_reason: "user" }, "Now: Off"],
      [{ mode: "continuous", mode_reason: null }, "Now: Continuous"],
    ];
    for (const [patch, expected] of statuses) {
      await sendStats(page, patch);
      await expect(page.getByTestId("mode-status")).toHaveText(expected);
    }
    expect(external).toEqual([]);
  });

  test("performance page: Download (480 MB) appears only for draft_unavailable and downloads the draft model", async ({ page }) => {
    await open(page, "control", { stats: { ...STATS_V2, mode: "light", mode_reason: "cpu" }, models: [...READY, DRAFT] });
    await page.getByRole("tab", { name: "Performance" }).click();
    const button = page.getByRole("button", { name: "Download (480 MB)" });
    await expect(button).toHaveCount(0);
    await sendStats(page, { mode: "light", mode_reason: "draft_unavailable" });
    await expect(button).toBeVisible();
    await button.click();
    await expect.poll(async () => (await calls(page, "models_download")).at(-1)?.args).toEqual({
      source: "huggingface",
      ids: ["lmt-60-0.6b-q4_k_m"],
    });
    await sendStats(page, { mode: "continuous", mode_reason: null });
    await expect(button).toHaveCount(0);
  });

  test("performance page: the three checkboxes save their keys and grey out per Caption speed", async ({ page }) => {
    await open(page, "control", { stats: STATS_V2 });
    await page.getByRole("tab", { name: "Performance" }).click();
    const games = page.getByRole("checkbox", { name: "Give games priority" });
    const slow = page.getByRole("checkbox", { name: "Slow down when the PC is busy" });
    const split = page.getByRole("checkbox", { name: "Split long sentences" });
    for (const box of [games, slow, split]) await expect(box).toBeChecked();
    await expect(page.locator("#help-low-priority")).toHaveText(
      "Runs translation at low priority so games stay smooth. Off: English can appear slightly sooner, but uses more CPU.",
    );
    await expect(page.locator("#help-step-down")).toHaveText("Switches to Light while the PC is busy and back when it is free.");
    await expect(page.locator("#help-split-long")).toHaveText("Translates long sentences in parts when the speaker does not pause.");

    await games.click();
    await expectPatch(page, { latency: { low_priority: false } });
    await expect(games).not.toBeChecked();
    await slow.click();
    await expectPatch(page, { latency: { step_down: false } });
    await split.click();
    await expectPatch(page, { latency: { split_long: false } });
    for (const box of [slow, split]) await box.click();

    const select = page.getByRole("combobox", { name: "Caption speed" });
    await select.selectOption("light");
    await expect(slow).toBeDisabled();
    await expect(split).toBeEnabled();
    await expect(games).toBeEnabled();
    await select.selectOption("off");
    await expect(slow).toBeDisabled();
    await expect(split).toBeDisabled();
    await expect(page.locator("#help-split-long")).toBeVisible();
    const greyed = await split.locator("xpath=ancestor::label").evaluate((el) => getComputedStyle(el).opacity);
    expect(greyed).toBe("0.5");
    const live = await games.locator("xpath=ancestor::label").evaluate((el) => getComputedStyle(el).opacity);
    expect(live).toBe("1");
    await select.selectOption("auto");
    await expect(slow).toBeEnabled();
    await expect(split).toBeEnabled();
  });

  test("performance page: the v2 stats and memory including the draft server", async ({ page }) => {
    await open(page, "control", { stats: STATS_V2 });
    await page.getByRole("tab", { name: "Performance" }).click();
    await expect(page.getByTestId("stat-first")).toHaveText("1.0 s");
    await expect(page.getByTestId("stat-final")).toHaveText("2.9 s");
    await expect(page.locator(".stat-grid")).toContainText("median, last minute");
    await expect(page.locator(".stat-grid")).not.toContainText("Speech end to English");
    await expect(page.getByTestId("stat-memory")).toHaveText("2.5 GB");
    await expect(page.locator(".stat-grid")).toContainText("CPU 98%, app and translators");
    await expect(page.getByRole("combobox", { name: "Translator threads" })).toBeVisible();
  });

  test("overlay page: Show Chinese while someone is speaking and Draft text", async ({ page }) => {
    await open(page, "control");
    await page.getByRole("tab", { name: "Overlay" }).click();
    const live = page.getByRole("checkbox", { name: "Show Chinese while someone is speaking" });
    await expect(live).toBeChecked();
    await live.click();
    await expectPatch(page, { overlay: { live_source: false } });
    await expect(live).not.toBeChecked();
    const draft = page.getByRole("combobox", { name: "Draft text" });
    await expect(draft.locator("option")).toHaveText(["Hold back the newest words", "Show only settled words", "Show everything"]);
    await expect(draft).toHaveValue("hold2");
    await expect(page.locator("#draft-display-help")).toHaveText(
      "Drafts are quick translations shown in a dimmer color until the final translation replaces them.",
    );
    await draft.selectOption("settled");
    await expectPatch(page, { overlay: { draft_display: "settled" } });
    await draft.selectOption("all");
    await expectPatch(page, { overlay: { draft_display: "all" } });
    await expect(page.getByRole("status")).toHaveCount(0);
  });

  test("first run: the draft model is optional on smaller PCs and included when recommended", async ({ page, browser }) => {
    const missing = READY.map((model, index) => (index === 0 ? model : { ...model, bytes_done: 0, state: "missing" as const }));
    await open(page, "control", { models: [...missing, DRAFT], state: { models_ready: false, listening: "paused" } });
    const box = page.getByRole("checkbox", { name: "Download Faster captions (LMT-60 0.6B, 480 MB)" });
    await expect(page.locator(".model").nth(3)).toContainText("Faster captions (LMT-60 0.6B, 480 MB)");
    await expect(page.locator(".model").nth(3)).toContainText("Optional");
    await expect(box).not.toBeChecked();
    await page.getByRole("button", { name: "Download" }).click();
    await expect.poll(async () => (await calls(page, "models_download")).at(-1)?.args).toEqual({ source: "huggingface" });
    await box.click();
    await page.getByRole("button", { name: "Download" }).click();
    await expect.poll(async () => (await calls(page, "models_download")).at(-1)?.args).toEqual({
      source: "huggingface",
      ids: ["asr", "mt", "lmt-60-0.6b-q4_k_m"],
    });

    const fast = await browser.newPage();
    await open(fast, "control", { models: [...missing, { ...DRAFT, recommended: true }], state: { models_ready: false, listening: "paused" } });
    await expect(fast.locator(".model").nth(3)).toContainText("Faster captions (LMT-60 0.6B, 480 MB)");
    await expect(fast.locator(".model input[type=checkbox]")).toHaveCount(0);
    await fast.getByRole("button", { name: "Download" }).click();
    await expect.poll(async () => (await calls(fast, "models_download")).at(-1)?.args).toEqual({ source: "huggingface" });
    await fast.close();
  });
});
