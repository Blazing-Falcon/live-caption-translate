import { expect, test, type Locator, type Page } from "@playwright/test";
import { calls, emitNamed, open } from "./support";

const MB = 1024 * 1024;
const MISSING = [
  { id: "vad", name: "Voice detection", bytes_total: 2 * MB, bytes_done: 2 * MB, optional: false, recommended: false, state: "ready" },
  { id: "asr", name: "Speech recognition", bytes_total: 239 * MB, bytes_done: 0, optional: false, recommended: false, state: "missing" },
  { id: "mt", name: "Translation", bytes_total: 1105 * MB, bytes_done: 0, optional: false, recommended: false, state: "missing" },
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
