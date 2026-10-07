import { expect, test, type Locator, type Page } from "@playwright/test";
import { calls, emitNamed, open } from "./support";

const MB = 1024 * 1024;
const MISSING = [
  { id: "vad", name: "Voice detection", bytes_total: 2 * MB, bytes_done: 2 * MB, state: "ready" },
  { id: "asr", name: "Speech recognition", bytes_total: 239 * MB, bytes_done: 0, state: "missing" },
  { id: "mt", name: "Translation", bytes_total: 1105 * MB, bytes_done: 0, state: "missing" },
] as const;

const expectPatch = (page: Page, patch: unknown): Promise<void> =>
  expect.poll(async () => (await calls(page, "set_config")).at(-1)?.args.patch).toEqual(patch);

const expectCalls = (page: Page, cmd: string, count: number): Promise<void> =>
  expect.poll(async () => (await calls(page, cmd)).length).toBe(count);

async function flip(locator: Locator, checked: boolean): Promise<void> {
  await locator.click();
  await expect(locator).toBeChecked({ checked });
}

test.describe("control window (browser simulation with mocked transport)", () => {
  test("renders the shell and makes no network request outside localhost", async ({ page }) => {
    const external = await open(page, "control");
    await expect(page.getByTestId("header-title")).toHaveText("Listening");
    await expect(page.getByTestId("footer")).toHaveText("speech→English 1.8 s median · queue 1 · CPU 41%");
    await expect(page.getByRole("tab")).toHaveText(["Capture", "Overlay", "Languages", "Performance"]);
    expect(external).toEqual([]);
  });

  test("is fully keyboard operable with a visible accent focus ring", async ({ page }) => {
    await open(page, "control");
    await page.keyboard.press("Tab");
    await expect(page.getByRole("tab", { name: "Capture" })).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("tab", { name: "Overlay" })).toBeFocused();
    await expect(page.getByRole("tab", { name: "Overlay" })).toHaveAttribute("aria-selected", "true");
    const outline = await page.getByRole("tab", { name: "Overlay" }).evaluate((el) => {
      const style = getComputedStyle(el);
      return { width: style.outlineWidth, style: style.outlineStyle, color: style.outlineColor };
    });
    expect(outline).toEqual({ width: "2px", style: "solid", color: "rgb(178, 115, 24)" });
    await page.keyboard.press("Tab");
    await expect(page.getByTestId("header-card").getByRole("button", { name: "Pause" })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(page.getByRole("radio", { name: "Subtitle bar" })).toBeFocused();
    await page.keyboard.press("Space");
    await page.keyboard.press("ArrowRight");
    await expectPatch(page, { overlay: { style: "panel" } });
  });

  test("controls are at least 36 px tall and app rows at least 44 px", async ({ page }) => {
    await open(page, "control", { config: { capture: { mode: "apps" } } });
    const small = await page.evaluate(() => {
      const bad: string[] = [];
      for (const el of document.querySelectorAll<HTMLElement>("button, select, input[type=text], input[type=range], .option-card, .check-row")) {
        const rect = el.getBoundingClientRect();
        if (rect.height > 0 && rect.height < 36) bad.push(`${el.tagName}.${el.className}:${rect.height}`);
      }
      return bad;
    });
    expect(small).toEqual([]);
    const rows = await page.locator(".list-row").evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().height));
    expect(rows.length).toBeGreaterThan(0);
    for (const height of rows) expect(height).toBeGreaterThanOrEqual(44);
  });

  test("follows the Windows light and dark scheme with the specified tokens", async ({ page }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await open(page, "control");
    const light = await page.evaluate(() => {
      const card = getComputedStyle(document.querySelector(".card") as Element);
      return { ground: getComputedStyle(document.body).backgroundColor, surface: card.backgroundColor, text: getComputedStyle(document.body).color };
    });
    expect(light).toEqual({ ground: "rgb(247, 247, 245)", surface: "rgb(255, 255, 255)", text: "rgb(27, 28, 30)" });
    await page.emulateMedia({ colorScheme: "dark" });
    const dark = await page.evaluate(() => {
      const card = getComputedStyle(document.querySelector(".card") as Element);
      return { ground: getComputedStyle(document.body).backgroundColor, surface: card.backgroundColor, text: getComputedStyle(document.body).color };
    });
    expect(dark).toEqual({ ground: "rgb(28, 29, 32)", surface: "rgb(37, 39, 42)", text: "rgb(237, 237, 234)" });
  });

  test("capture page saves mode, device and app picks", async ({ page }) => {
    await open(page, "control");
    await flip(page.getByRole("radio", { name: /Selected apps/ }), true);
    await expectPatch(page, { capture: { mode: "apps" } });
    await expect(page.locator(".list-row").first()).toContainText("Google Chrome");
    await expect(page.locator(".list-row").first()).toContainText("-6 dB");
    await flip(page.getByRole("checkbox", { name: /Google Chrome/ }), true);
    await expectPatch(page, { capture: { apps: [{ exe: "chrome.exe", name: "Google Chrome" }] } });
    await flip(page.getByRole("radio", { name: /Whole system/ }), true);
    await page.getByRole("combobox", { name: "Output device" }).selectOption("dev-usb");
    await expectPatch(page, { capture: { device: "dev-usb" } });
  });

  test("selected apps are disabled with the reason on unsupported Windows", async ({ page }) => {
    await open(page, "control", {
      config: { capture: { mode: "apps" } },
      apps: { supported: false, reason: "Selected apps needs Windows 11 (build 20348 or later).", apps: [] },
    });
    await expect(page.getByRole("radio", { name: /Selected apps/ })).toBeDisabled();
    await expect(page.getByTestId("apps-unsupported")).toContainText("build 20348 or later");
  });

  test("overlay page round-trips style, sliders, source toggle, hotkeys and visibility", async ({ page }) => {
    await open(page, "control");
    await page.getByRole("tab", { name: "Overlay" }).click();
    await page.getByLabel("Background", { exact: false }).first().fill("30");
    await expectPatch(page, { overlay: { background: 0.3 } });
    await expect(page.getByTestId("background-warning")).toHaveText("May be hard to read on bright video");
    await page.getByLabel(/English text size/).fill("34");
    await expectPatch(page, { overlay: { font_px: 34 } });
    await flip(page.getByRole("checkbox", { name: /Show the Chinese line/ }), false);
    await expectPatch(page, { overlay: { show_source: false } });
    await page.getByRole("textbox", { name: "Pause listening" }).fill("Ctrl+Alt+P");
    await page.getByRole("button", { name: "Save Pause listening shortcut" }).click();
    await expectPatch(page, { hotkeys: { pause: "Ctrl+Alt+P" } });
    await page.getByRole("textbox", { name: "Show or hide overlay" }).fill("");
    await page.getByRole("button", { name: "Save Show or hide overlay shortcut" }).click();
    await expectPatch(page, { hotkeys: { show_hide: "" } });
    await page.getByRole("button", { name: "Move overlay" }).click();
    await expect.poll(async () => (await calls(page, "set_overlay_moving")).at(-1)?.args).toEqual({ moving: true });
    await emitNamed(page, "hotkeys://error", { action: "move_lock", accelerator: "Ctrl+Shift+L", message: "Another app is using it." });
    await expect(page.getByRole("alert")).toContainText("Move or lock overlay: Ctrl+Shift+L could not be used. Another app is using it.");
    await expect(page.getByRole("tab", { name: "Overlay" })).toBeVisible();
  });

  test("languages and performance pages round-trip their settings", async ({ page }) => {
    await open(page, "control", { config: { routing: { translate_other: ["yue"] } } });
    await page.getByRole("tab", { name: "Languages" }).click();
    await flip(page.getByRole("checkbox", { name: "Japanese" }), true);
    await expectPatch(page, { routing: { translate_other: ["yue", "ja"] } });
    await page.getByRole("tab", { name: "Performance" }).click();
    await expect(page.getByTestId("stat-median")).toHaveText("1.8 s");
    await page.getByRole("combobox", { name: "Translator threads" }).selectOption("3");
    await expectPatch(page, { translate: { threads: 3 } });
    await flip(page.getByRole("checkbox", { name: /Save a transcript/ }), false);
    await expectPatch(page, { transcript: { enabled: false } });
    await page.getByRole("button", { name: "Open transcript folder" }).click();
    await expect.poll(async () => (await calls(page, "open_folder")).at(-1)?.args).toEqual({ which: "transcripts" });
  });

  test("header pause/start follows the pipeline state events", async ({ page }) => {
    await open(page, "control");
    await page.getByRole("button", { name: "Pause" }).click();
    await expectCalls(page, "pause_listening", 1);
    await emitNamed(page, "pipeline://event", { type: "listening_state", state: "paused" });
    await expect(page.getByTestId("header-title")).toHaveText("Paused");
    await expect(page.getByTestId("footer")).toHaveText("paused");
    await page.getByRole("button", { name: "Start" }).click();
    await expectCalls(page, "start_listening", 1);
  });

  test("first run: download, pause, resume, corruption retry and the single start", async ({ page }) => {
    await open(page, "control", { models: [...MISSING], state: { models_ready: false, listening: "paused" } });
    await expect(page.getByRole("heading", { name: "Download the speech and translation models" })).toBeVisible();
    await page.getByRole("button", { name: "Download" }).click();
    await expect.poll(async () => (await calls(page, "models_download")).at(-1)?.args).toEqual({ source: "huggingface" });
    const progress = (state: string, done: number) => [
      MISSING[0],
      { ...MISSING[1], bytes_done: done, state },
      MISSING[2],
    ];
    await emitNamed(page, "models://progress", progress("downloading", 152 * MB));
    await expect(page.locator(".model").nth(1)).toContainText("152 MB / 239 MB");
    await expect(page.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "64");
    await page.getByRole("button", { name: "Pause" }).click();
    await expectCalls(page, "models_pause", 1);
    await emitNamed(page, "models://progress", progress("paused", 152 * MB));
    await page.getByRole("button", { name: "Resume" }).click();
    await emitNamed(page, "models://progress", [MISSING[0], { ...MISSING[1], bytes_done: 239 * MB, state: "corrupt" }, MISSING[2]]);
    await expect(page.locator(".model").nth(1)).toContainText("Damaged");
    await page.getByRole("button", { name: "Retry" }).click();
    const ready = MISSING.map((model) => ({ ...model, bytes_done: model.bytes_total, state: "ready" }));
    await emitNamed(page, "models://progress", ready);
    await expect(page.getByRole("tab", { name: "Capture" })).toHaveAttribute("aria-selected", "true");
    await emitNamed(page, "models://progress", ready);
    await expectCalls(page, "start_listening", 1);
  });
});
