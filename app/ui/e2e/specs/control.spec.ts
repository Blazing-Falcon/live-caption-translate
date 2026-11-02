import { expect, test, type Page } from "@playwright/test";
import type { ModelStatus } from "../../src/lib/types";
import { calls, emitNamed, open } from "./support";

const MB = 1024 * 1024;
const SPEECH = { id: "xasr-stream-480ms-int8-2026-06-05", name: "Speech recognition", bytes_total: 303 * MB, bytes_done: 303 * MB, optional: false, state: "ready" } as const;
const SECOND = { id: "xasr-offline-int8-2026-06-03", name: "Second-pass speech recognition", bytes_total: 311 * MB, bytes_done: 311 * MB, optional: false, state: "ready" } as const;
const TRANSLATOR = { id: "hy-mt2-1.8b-ternary-tq2_0", name: "Translation", bytes_total: 1068 * MB, bytes_done: 1068 * MB, optional: false, state: "ready" } as const;
const OCR = { id: "pp-ocrv6-det-tiny-rec-small", name: "Screen text recognition", bytes_total: 23 * MB, bytes_done: 0, optional: true, state: "missing" } as const;

const lastPatch = async (page: Page): Promise<unknown> => (await calls(page, "set_config")).at(-1)?.args.patch;
const expectPatch = (page: Page, patch: unknown): Promise<void> => expect.poll(() => lastPatch(page)).toEqual(patch);
const expectCalls = (page: Page, cmd: string, count: number): Promise<void> =>
  expect.poll(async () => (await calls(page, cmd)).length).toBe(count);
const openPage = (page: Page, name: string): Promise<void> => page.getByRole("tab", { name }).click();

test.describe("control window (browser simulation with mocked transport)", () => {
  test("renders the shell, says what is happening and makes no network request outside localhost", async ({ page }) => {
    const external = await open(page, "control");
    await expect(page.getByRole("tab")).toHaveText(["Home", "Captions", "Listening", "Translation", "Screenshot", "Shortcuts", "History", "About & help"]);
    await expect(page.getByTestId("header-title")).toHaveText("Listening to your PC");
    await expect(page.getByTestId("health")).toContainText("Skipped 3 lines to catch up");
    await page.getByRole("button", { name: "Pause" }).click();
    await expectCalls(page, "pause_listening", 1);
    expect(external).toEqual([]);
  });

  test("first start: the online path saves the choices, downloads, and starts listening once", async ({ page }) => {
    const missing: ModelStatus[] = [
      { ...SPEECH, bytes_done: 0, state: "missing" },
      { ...SECOND, optional: true, bytes_done: 0, state: "missing" },
      { ...TRANSLATOR, optional: true, bytes_done: 0, state: "missing" },
    ];
    await open(page, "control", { models: missing, state: { models_ready: false, listening: "paused" } });
    await expect(page.getByRole("heading", { name: "Hi! Let's set up your captions." })).toBeVisible();
    await expect(page.getByRole("radio", { name: /Free online service/ })).toBeChecked();
    await page.getByRole("button", { name: "Next" }).click();
    await page.getByRole("radio", { name: /Not now/ }).check();
    await page.getByRole("button", { name: "Next" }).click();
    await expect(page.getByTestId("download-total")).toContainText("303 MB");
    await page.getByRole("button", { name: "Download and start" }).click();
    await expectPatch(page, { translate: { engine: "microsoft" }, asr: { second_pass: false }, capture: { mode: "system" } });
    await expectCalls(page, "models_download", 1);
    const speech = (state: ModelStatus["state"], done: number) => [{ ...SPEECH, state, bytes_done: done }, missing[1], missing[2]];
    await emitNamed(page, "models://progress", speech("downloading", 101 * MB));
    await expect(page.locator(".model")).toHaveCount(1);
    await expect(page.locator(".model")).toContainText("Downloading · 33%");
    await page.getByRole("button", { name: "Pause" }).click();
    await expectCalls(page, "models_pause", 1);
    await emitNamed(page, "models://progress", speech("corrupt", 303 * MB));
    await page.getByRole("button", { name: "Retry" }).click();
    await expectCalls(page, "models_download", 2);
    await emitNamed(page, "models://progress", speech("ready", 303 * MB));
    await expect(page.getByTestId("header-title")).toBeVisible();
    await emitNamed(page, "models://progress", speech("ready", 303 * MB));
    await expectCalls(page, "start_listening", 1);
  });

  test("a stacked row puts its control right under its label", async ({ page }) => {
    await open(page, "control");
    await openPage(page, "Captions");
    const row = page.getByTestId("english-color");
    const help = await row.locator(".help").boundingBox();
    const control = await row.locator(".control").boundingBox();
    expect(control!.y - (help!.y + help!.height)).toBeLessThan(16);
  });

  test("translator choice names who sees the text and switches online services", async ({ page }) => {
    await open(page, "control", { config: { translate: { engine: "microsoft" } } });
    await openPage(page, "Translation");
    const note = page.getByTestId("privacy-note");
    await expect(note).toContainText("Microsoft gets the Chinese text to translate. Audio and screenshots stay on this PC. If Microsoft doesn't answer, Google may see the text too.");
    await page.getByRole("switch", { name: "Use a backup service" }).click();
    await expectPatch(page, { translate: { fallback: false } });
    await expect(note).not.toContainText("doesn't answer");
    await page.getByTestId("translate-engine").selectOption("google");
    await expectPatch(page, { translate: { engine: "google" } });
    await expect(note).toContainText("Google gets the Chinese text to translate.");
    await page.getByRole("radio", { name: /My own AI server/ }).check();
    await expectPatch(page, { translate: { engine: "openai" } });
    await expect(note).toContainText("Your server gets the Chinese text.");
    await page.getByRole("radio", { name: /Free online service/ }).check();
    await expectPatch(page, { translate: { engine: "google" } });
  });

  test("on this PC downloads the translator first and switches only when it is ready", async ({ page }) => {
    const missing = { ...TRANSLATOR, optional: true, bytes_done: 0, state: "missing" } as const;
    await open(page, "control", { config: { translate: { engine: "microsoft" } }, models: [SPEECH, SECOND, missing] });
    await openPage(page, "Translation");
    await page.getByRole("radio", { name: /On this PC/ }).check();
    await expect.poll(async () => (await calls(page, "models_download_optional")).at(-1)?.args).toEqual({ id: TRANSLATOR.id });
    await expect(page.getByText("Until it finishes, captions keep using Microsoft.", { exact: false })).toBeVisible();
    await emitNamed(page, "models://progress", [SPEECH, SECOND, { ...missing, state: "downloading", bytes_done: 534 * MB }]);
    await expect(page.getByRole("progressbar", { name: "Translator download" })).toHaveAttribute("aria-valuenow", "50");
    expect(await calls(page, "set_config")).toEqual([]);
    await emitNamed(page, "models://progress", [SPEECH, SECOND, { ...missing, state: "ready", bytes_done: missing.bytes_total }]);
    await expectPatch(page, { translate: { engine: "local" } });
    await expect(page.getByTestId("privacy-note")).toContainText("Everything stays on this PC.");
  });

  test("OpenAI-compatible settings mask the key and test saved settings", async ({ page }) => {
    await open(page, "control", { config: { translate: { engine: "openai" } } });
    await openPage(page, "Translation");
    for (const [label, key, value] of [
      ["Address", "base_url", "http://localhost:1234/v1"],
      ["Model name", "model", "test-model"],
      ["API key", "api_key", "test-secret"],
      ["Instruction", "prompt", "Translate into English."],
    ] as const) {
      const input = page.getByLabel(label, { exact: true });
      await input.fill(value);
      await input.press("Tab");
      await expectPatch(page, { translate: { openai: { [key]: value } } });
    }
    await expect(page.getByLabel("API key", { exact: true })).toHaveAttribute("type", "password");
    await page.getByRole("button", { name: "Test", exact: true }).click();
    await expect(page.getByTestId("translator-test-result")).toHaveText("Today’s meeting has been moved to 3 p.m. (123 ms)");
    await page.evaluate(() => window.__lt.backend.failures.set("test_translator", "The endpoint rejected the API key."));
    await page.getByRole("button", { name: "Test", exact: true }).click();
    await expect(page.getByTestId("translator-test-result")).toHaveText("The endpoint rejected the API key.");
  });

  test("listening page saves mode, app picks and device, and turns double-check off and back on", async ({ page }) => {
    const missing = { ...SECOND, optional: true, bytes_done: 0, state: "missing" } as const;
    await open(page, "control", { models: [SPEECH, missing, TRANSLATOR] });
    await openPage(page, "Listening");
    await page.getByRole("radio", { name: /Only the apps I pick/ }).check();
    await expectPatch(page, { capture: { mode: "apps" } });
    await page.getByRole("checkbox", { name: /Google Chrome/ }).check();
    await expectPatch(page, { capture: { apps: [{ exe: "chrome.exe", name: "Google Chrome" }] } });
    await page.getByRole("radio", { name: /Everything on this PC/ }).check();
    await page.getByRole("combobox", { name: "Speaker or headset" }).selectOption("dev-usb");
    await expectPatch(page, { capture: { device: "dev-usb" } });

    const doubleCheck = page.getByRole("switch", { name: "Double-check each line" });
    await doubleCheck.click();
    await expectPatch(page, { asr: { second_pass: false } });
    await expect(doubleCheck).toHaveAttribute("aria-checked", "false");
    await doubleCheck.click();
    await expect.poll(async () => (await calls(page, "models_download_optional")).at(-1)?.args).toEqual({ id: SECOND.id });
    await expect(doubleCheck).toHaveAttribute("aria-checked", "true");
    await expect(lastPatch(page)).resolves.toEqual({ asr: { second_pass: false } });
    await emitNamed(page, "models://progress", [SPEECH, { ...missing, state: "ready", bytes_done: missing.bytes_total }, TRANSLATOR]);
    await expectPatch(page, { asr: { second_pass: true } });
  });

  test("caption colors warn when hard to read on the night card, and the outline card needs no warning", async ({ page }) => {
    await open(page, "control");
    await openPage(page, "Captions");
    await page.getByRole("radio", { name: "Pencil yellow" }).check();
    await expectPatch(page, { overlay: { english_color: "#ffd84d" } });
    await page.getByTestId("chinese-color").getByLabel("Custom color").evaluate((input: HTMLInputElement) => {
      input.value = "#3a3a8a";
      input.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await expectPatch(page, { overlay: { chinese_color: "#3a3a8a" } });
    await expect(page.getByTestId("chinese-color")).toContainText("Hard to read on the dark card (1.7:1). Pick a lighter color.");
    await page.getByRole("radio", { name: "Outline" }).check();
    await expectPatch(page, { overlay: { card: "outline" } });
    await expect(page.getByText(/Hard to read on the dark card/)).toHaveCount(0);
    await expect(page.getByLabel("Card see-through")).toHaveCount(0);
    await page.getByRole("button", { name: "Reset colors" }).click();
    await expectPatch(page, { overlay: { english_color: "#ffffff", chinese_color: "#c9c9c4" } });
  });

  test("shortcuts are recorded from key presses and a refused one is explained", async ({ page }) => {
    await open(page, "control");
    await openPage(page, "Shortcuts");
    await page.getByRole("button", { name: "Change Translate the screen" }).click();
    await page.keyboard.press("Control+Alt+KeyT");
    await expectPatch(page, { hotkeys: { screen_translate: "Ctrl+Alt+T" } });
    await emitNamed(page, "hotkeys://error", { action: "screen_translate", accelerator: "Ctrl+Alt+T", message: "Another program uses it." });
    await expect(page.getByTestId("hotkey-screen_translate").getByRole("alert")).toHaveText("Ctrl+Alt+T could not be used. Another program uses it.");
    await page.getByRole("button", { name: "Change Pause or start listening" }).click();
    await page.keyboard.press("Backspace");
    await expectPatch(page, { hotkeys: { pause: "" } });
    await expect(page.getByTestId("hotkey-pause")).toContainText("Off");
  });

  test("screenshot page downloads the text reader and shows its progress", async ({ page }) => {
    await open(page, "control", { models: [SPEECH, SECOND, TRANSLATOR, OCR] });
    await openPage(page, "Screenshot");
    const row = page.getByTestId("screen-model");
    await expect(row).toContainText("Not downloaded · 23 MB");
    await row.getByRole("button", { name: "Download" }).click();
    await expect.poll(async () => (await calls(page, "models_download_optional")).at(-1)?.args).toEqual({ id: OCR.id });
    await emitNamed(page, "models://progress", [SPEECH, SECOND, TRANSLATOR, { ...OCR, state: "downloading", bytes_done: 11.5 * MB }]);
    await expect(row.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "50");
    await expect(row.getByRole("button")).toHaveCount(0);
    await emitNamed(page, "models://progress", [SPEECH, SECOND, TRANSLATOR, { ...OCR, state: "ready", bytes_done: OCR.bytes_total }]);
    await expect(row).toContainText("Ready");
  });

  test("history lists saved sessions and reads one", async ({ page }) => {
    await open(page, "control");
    await openPage(page, "History");
    const list = page.getByTestId("transcripts");
    await expect(list).toContainText("1 Nov 2026, 21:12");
    await expect(list).toContainText("Speakers (Realtek) · 2 lines");
    await list.getByRole("button", { name: "Read" }).click();
    await expect.poll(async () => (await calls(page, "read_transcript")).at(-1)?.args).toEqual({ file: "2026-11-01_211200.jsonl" });
    const reader = page.getByTestId("transcript-reader");
    await expect(reader.locator(".line")).toHaveCount(2);
    await expect(reader.locator(".line").first()).toContainText(["21:12:05", "你昨天晚上去哪儿了？", "Where were you last night?"].join(" "));
    await expect(reader.locator(".line").last()).toContainText("Skipped to catch up");
    await reader.getByRole("button", { name: "Back to the list" }).click();
    await expect(page.getByTestId("transcripts")).toBeVisible();
  });

  test("the app theme setting overrides the system theme", async ({ page }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await open(page, "control");
    await openPage(page, "About & help");
    await page.getByRole("radio", { name: "Dark" }).check();
    await expectPatch(page, { app: { theme: "dark" } });
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await page.getByRole("radio", { name: "Like Windows" }).check();
    await expect(page.locator("html")).not.toHaveAttribute("data-theme", /./);
  });
});
