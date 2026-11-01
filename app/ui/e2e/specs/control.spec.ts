import { expect, test, type Locator, type Page } from "@playwright/test";
import { calls, emitNamed, open } from "./support";

const MB = 1024 * 1024;
const MISSING = [
  { id: "hy-mt2-1.8b-ternary-tq2_0", name: "Translation", bytes_total: 578 * MB, bytes_done: 578 * MB, optional: false, state: "ready" },
  { id: "xasr-stream-480ms-int8-2026-06-05", name: "Speech recognition", bytes_total: 128 * MB, bytes_done: 0, optional: false, state: "missing" },
  { id: "xasr-offline-int8-2026-06-03", name: "Second-pass speech recognition", bytes_total: 130 * MB, bytes_done: 0, optional: false, state: "missing" },
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
  test("OpenAI-compatible settings mask the key and test saved settings", async ({ page }) => {
    await open(page, "control", { config: { translate: { engine: "openai" } } });
    await page.getByRole("tab", { name: "Performance" }).click();
    for (const [label, key, value] of [
      ["Base URL", "base_url", "http://localhost:1234/v1"],
      ["Model", "model", "test-model"],
      ["API key", "api_key", "test-secret"],
      ["Translation prompt", "prompt", "Translate into English."],
    ] as const) {
      const input = page.getByLabel(label, { exact: true });
      await input.fill(value);
      await input.press("Tab");
      await expectPatch(page, { translate: { openai: { [key]: value } } });
    }
    await expect(page.getByLabel("API key", { exact: true })).toHaveAttribute("type", "password");
    await page.getByRole("button", { name: "Test translator", exact: true }).click();
    await expectCalls(page, "test_translator", 1);
    await expect(page.getByTestId("translator-test-result")).toHaveText("Today’s meeting has been moved to 3 p.m. (123 ms)");
    await page.evaluate(() => window.__lt.backend.failures.set("test_translator", "The endpoint rejected the API key."));
    await page.getByRole("button", { name: "Test translator", exact: true }).click();
    await expect(page.getByTestId("translator-test-result")).toHaveText("The endpoint rejected the API key.");
  });

  test("translator choice saves one engine and explains where text goes", async ({ page }) => {
    await open(page, "control");
    await page.getByRole("tab", { name: "Performance" }).click();
    const select = page.getByTestId("translate-engine");
    await expect(select).toHaveValue("local");
    await select.selectOption("microsoft");
    await expectPatch(page, { translate: { engine: "microsoft" } });
    await expect(page.getByText("Captions and screenshot text are sent to Microsoft to be translated.")).toBeVisible();
    await expect(page.getByText(/CPU .*app and translation server/)).toHaveCount(0);
    await select.selectOption("google");
    await expectPatch(page, { translate: { engine: "google" } });
    await expect(page.getByText("Captions and screenshot text are sent to Google to be translated.")).toBeVisible();
    await select.selectOption("local");
    await expectPatch(page, { translate: { engine: "local" } });
    await expect(page.getByText(/Captions and screenshot text are sent/)).toHaveCount(0);
  });

  test("renders the shell and makes no network request outside localhost", async ({ page }) => {
    const external = await open(page, "control");
    await expect(page.getByTestId("header-title")).toHaveText("Listening");
    await expect(page.getByTestId("footer")).toHaveText("speech→English 1.8 s median · queue 1 · CPU 41%");
    await expect(page.getByRole("tab")).toHaveText(["Capture", "Overlay", "Performance"]);
    expect(external).toEqual([]);
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

  test("screen translation: the shortcut field saves and the text recognition model downloads", async ({ page }) => {
    const READY = MISSING.map((model) => ({ ...model, bytes_done: model.bytes_total, state: "ready" as const }));
    const OCR = { id: "pp-ocrv6-det-tiny-rec-small", name: "Screen text recognition", bytes_total: 22 * MB, bytes_done: 0, optional: true, state: "missing" } as const;
    await open(page, "control", { models: [...READY, OCR] });

    await page.getByRole("tab", { name: "Overlay" }).click();
    const shortcut = page.getByLabel("Translate the screen", { exact: true });
    await expect(shortcut).toHaveValue("Ctrl+Shift+A");
    await shortcut.fill("Ctrl+Alt+T");
    await page.getByRole("button", { name: "Save Translate the screen shortcut" }).click();
    await expectPatch(page, { hotkeys: { screen_translate: "Ctrl+Alt+T" } });
    await emitNamed(page, "hotkeys://error", { action: "screen_translate", accelerator: "Ctrl+Alt+T", message: "Another program uses it." });
    await expect(page.getByRole("alert")).toContainText("Translate the screen: Ctrl+Alt+T could not be used");

    await page.getByRole("tab", { name: "Performance" }).click();
    const row = page.getByTestId("screen-model");
    await expect(row).toContainText("22 MB");
    await expect(row).toContainText("Not downloaded");
    await row.getByRole("button", { name: "Download" }).click();
    await expect.poll(async () => (await calls(page, "models_download_optional")).at(-1)?.args).toEqual({ id: OCR.id });
    await emitNamed(page, "models://progress", [...READY, { ...OCR, state: "downloading", bytes_done: 11 * MB }]);
    await expect(row).toContainText("Downloading · 50%");
    await expect(row.getByRole("progressbar")).toHaveAttribute("value", "50");
    await expect(row.getByRole("button")).toHaveCount(0);
    await emitNamed(page, "models://progress", [...READY, { ...OCR, state: "ready", bytes_done: OCR.bytes_total }]);
    await expect(row).toContainText("Ready");
    await expect(row.getByRole("button")).toHaveCount(0);
  });

  test("first run: download, pause, resume, corruption retry and the single start", async ({ page }) => {
    await open(page, "control", { models: [...MISSING], state: { models_ready: false, listening: "paused" } });
    await expect(page.getByRole("heading", { name: "Download the speech and translation models" })).toBeVisible();
    await page.getByRole("button", { name: "Download" }).click();
    await expect.poll(async () => (await calls(page, "models_download")).at(-1)?.args).toEqual({});
    const progress = (state: string, done: number) => [
      MISSING[0],
      { ...MISSING[1], bytes_done: done, state },
      MISSING[2],
    ];
    await emitNamed(page, "models://progress", progress("downloading", 82 * MB));
    await expect(page.locator(".model").nth(1)).toContainText("82 MB / 128 MB");
    await expect(page.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "64");
    await page.getByRole("button", { name: "Pause" }).click();
    await expectCalls(page, "models_pause", 1);
    await emitNamed(page, "models://progress", progress("paused", 82 * MB));
    await page.getByRole("button", { name: "Resume" }).click();
    await emitNamed(page, "models://progress", [MISSING[0], { ...MISSING[1], bytes_done: 128 * MB, state: "corrupt" }, MISSING[2]]);
    await expect(page.locator(".model").nth(1)).toContainText("Damaged");
    await page.getByRole("button", { name: "Retry" }).click();
    const ready = MISSING.map((model) => ({ ...model, bytes_done: model.bytes_total, state: "ready" }));
    await emitNamed(page, "models://progress", ready);
    await expect(page.getByRole("tab", { name: "Capture" })).toHaveAttribute("aria-selected", "true");
    await emitNamed(page, "models://progress", ready);
    await expectCalls(page, "start_listening", 1);
  });
});
