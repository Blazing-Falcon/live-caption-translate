import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";
import type { Boot } from "../harness/boot";
import type { ModelStatus, ScreenBlock, ScreenEvent, ScreenFrame } from "../../src/lib/types";
import { calls, emitNamed, open } from "./support";

const SCALE = 1.5;
const FRAME_PNG = fileURLToPath(new URL("../../../../testdata/screen/article@1.5.png", import.meta.url));
const recorded = JSON.parse(
  readFileSync(new URL("../fixtures/article@1.5.screen.json", import.meta.url), "utf8"),
) as { events: { event: ScreenEvent }[] };

/** The events `lt-cli screen` recorded for the frame, as job `job`. */
const job = (id: number): ScreenEvent[] => recorded.events.map(({ event }) => ({ ...event, job: id }));
const blocks = (): ScreenBlock[] => {
  const event = recorded.events.map((entry) => entry.event).find((entry) => entry.type === "blocks");
  if (event?.type !== "blocks") throw new Error("The fixture has no blocks event");
  return event.blocks;
};
const ofType = (id: number, ...types: ScreenEvent["type"][]): ScreenEvent[] =>
  job(id).filter((event) => types.includes(event.type));

const FRAME: ScreenFrame = { id: 7, width: 1200, height: 720, scale: SCALE };
const OCR_MODEL: ModelStatus = {
  id: "pp-ocrv6-det-tiny-rec-small",
  name: "Screen text recognition",
  bytes_total: 24_000_000,
  bytes_done: 0,
  optional: true,
  state: "missing",
};

test.use({
  deviceScaleFactor: SCALE,
  viewport: { width: FRAME.width / SCALE, height: FRAME.height / SCALE },
  permissions: ["clipboard-read", "clipboard-write"],
});

async function start(page: Page, boot: Boot = {}): Promise<void> {
  await page.route("http://ltshot.localhost/**", (route) => route.fulfill({ path: FRAME_PNG }));
  await open(page, "screen", boot);
  await emitNamed(page, "screen://frame", FRAME);
  await expect.poll(() => page.locator(".frame").evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(FRAME.width);
}

async function drag(page: Page, from: { x: number; y: number }, to: { x: number; y: number }): Promise<void> {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2);
  await page.mouse.move(to.x, to.y);
  await page.mouse.up();
}

/** Windows puts CRLF line breaks on the clipboard. */
const clipboard = async (page: Page): Promise<string> =>
  (await page.evaluate(() => navigator.clipboard.readText())).replaceAll("\r\n", "\n");

async function send(page: Page, events: ScreenEvent[]): Promise<void> {
  for (const event of events) await emitNamed(page, "screen://event", event);
}

/** Covers both recorded blocks, and leaves the top-left corner free for a second drag. */
const AREA = { from: { x: 40, y: 30 }, to: { x: 760, y: 440 } };

test.describe("screenshot window (frame and events recorded by lt-cli at scale 1.5)", () => {
  test("selects in frame pixels and paints each block at rect / scale once it is translated", async ({ page }) => {
    await start(page);
    const box = await page.locator(".frame").boundingBox();
    expect(box).toMatchObject({ x: 0, y: 0, width: 800, height: 480 });

    await drag(page, { x: 100, y: 100 }, { x: 105, y: 400 });
    await drag(page, { x: 100, y: 100 }, { x: 400, y: 106 });
    expect(await calls(page, "screen_select")).toEqual([]);

    await drag(page, AREA.from, AREA.to);
    expect(await calls(page, "screen_select")).toEqual([
      { cmd: "screen_select", args: { frame: FRAME.id, rect: { x: 60, y: 45, w: 1080, h: 615 } } },
    ]);
    await expect(page.getByRole("status")).toHaveText("Reading text…");

    await send(page, ofType(1, "reading", "blocks"));
    await expect(page.getByRole("status")).toHaveText("Translating 0 of 2…");
    await expect(page.locator("[data-block]")).toHaveCount(0);

    const [first, second] = blocks();
    if (!first || !second) throw new Error("The fixture should have two blocks");
    const translated = ofType(1, "translated");
    await send(page, translated.slice(0, 1));
    await expect(page.locator("[data-block]")).toHaveCount(1);
    await expect(page.getByRole("status")).toHaveText("Translating 1 of 2…");
    await send(page, [...translated.slice(1), ...ofType(1, "done")]);
    await expect(page.locator("[data-block]")).toHaveCount(2);
    await expect(page.getByRole("status")).toHaveText("Done.");

    for (const block of [first, second]) {
      const painted = await page.locator(`[data-block="${block.id}"]`).boundingBox();
      expect(painted?.x).toBeCloseTo(block.rect.x / SCALE, 0);
      expect(painted?.y).toBeCloseTo(block.rect.y / SCALE, 0);
      expect(painted?.width).toBeCloseTo(block.rect.w / SCALE, 0);
      expect(painted?.height).toBeCloseTo(block.rect.h / SCALE, 0);
    }
    const overflow = await page
      .locator("[data-block]")
      .evaluateAll((elements) => elements.map((element) => element.scrollHeight - element.clientHeight));
    expect(overflow).toHaveLength(2);
    expect(Math.max(...overflow)).toBeLessThanOrEqual(1);
    const colors = await page
      .locator('[data-block="0"]')
      .evaluate((element) => ({ background: getComputedStyle(element).backgroundColor, color: getComputedStyle(element).color }));
    expect(colors).toEqual({ background: "rgb(251, 250, 247)", color: "rgb(24, 24, 24)" });
  });

  test("Escape and a right click call screen_close", async ({ page }) => {
    await start(page);
    await page.keyboard.press("Escape");
    expect(await calls(page, "screen_close")).toHaveLength(1);
    await expect(page.locator(".screen")).toHaveCount(0);

    await emitNamed(page, "screen://frame", { ...FRAME, id: 8 });
    await expect(page.locator(".screen")).toHaveCount(1);
    await page.mouse.click(300, 200, { button: "right" });
    expect(await calls(page, "screen_close")).toHaveLength(2);
    await expect(page.locator(".screen")).toHaveCount(0);
  });

  test("a second drag starts a new job and ignores the late events of the first", async ({ page }) => {
    await start(page);
    await drag(page, AREA.from, AREA.to);
    await send(page, [...ofType(1, "reading", "blocks"), ...ofType(1, "translated").slice(0, 1)]);
    await expect(page.locator("[data-block]")).toHaveCount(1);

    await drag(page, { x: 20, y: 10 }, { x: 300, y: 200 });
    expect(await calls(page, "screen_select")).toHaveLength(2);
    await expect(page.locator("[data-block]")).toHaveCount(0);
    await expect(page.getByRole("status")).toHaveText("Reading text…");

    await send(page, [...ofType(1, "translated").slice(1), ...ofType(1, "done")]);
    await expect(page.locator("[data-block]")).toHaveCount(0);
    await expect(page.getByRole("status")).toHaveText("Reading text…");

    await send(page, job(2));
    await expect(page.locator("[data-block]")).toHaveCount(2);
    await expect(page.getByRole("status")).toHaveText("Done.");
  });

  test("marks a block that failed and says why", async ({ page }) => {
    await start(page);
    await drag(page, AREA.from, AREA.to);
    const [, translated] = ofType(1, "translated");
    if (!translated) throw new Error("The fixture should translate two blocks");
    await send(page, [
      ...ofType(1, "reading", "blocks"),
      { type: "failed", job: 1, id: 0, message: "Translation took too long" },
      translated,
      ...ofType(1, "done"),
    ]);
    await expect(page.locator("[data-block]")).toHaveCount(1);
    const mark = page.locator('[data-failed="0"]');
    await expect(mark).toHaveAttribute("title", "Translation took too long");
    await expect(page.getByRole("status")).toHaveText("Done. 1 of 2 could not be translated.");
    // The native tooltip may sit behind the always-on-top window, so the status line says it too.
    await mark.hover();
    await expect(page.getByRole("status")).toHaveText("Translation took too long");
    await page.mouse.move(700, 20);
    await expect(page.getByRole("status")).toHaveText("Done. 1 of 2 could not be translated.");
  });

  test("widens a narrow box for a word that does not fit and never breaks the word", async ({ page }) => {
    await start(page);
    await drag(page, AREA.from, AREA.to);
    const rect = { x: 300, y: 300, w: 60, h: 33 };
    const label: ScreenBlock = {
      id: 0,
      rect,
      lines: [rect],
      line_height: rect.h,
      source: "应用",
      background: [240, 240, 240],
      foreground: [24, 24, 24],
      translate: true,
    };
    await send(page, [
      { type: "reading", job: 1 },
      { type: "blocks", job: 1, ocr_ms: 1, blocks: [label] },
      { type: "translated", job: 1, id: 0, text: "Application" },
    ]);
    const painted = await page.locator('[data-block="0"]').boundingBox();
    expect(painted?.width).toBeGreaterThan(rect.w / SCALE + 4);
    expect(painted?.height).toBeLessThanOrEqual(rect.h / SCALE + 1);
  });

  test("copies the English or the Chinese, and shows the original while held", async ({ page }) => {
    await start(page);
    await drag(page, AREA.from, AREA.to);
    await send(page, job(1));
    const sources = blocks().map((block) => block.source);
    const english = ofType(1, "translated").map((event) => (event.type === "translated" ? event.text : ""));

    await page.getByRole("button", { name: "Copy English" }).click();
    await expect(page.getByRole("button", { name: "Copied" })).toBeVisible();
    expect(await clipboard(page)).toBe(english.join("\n\n"));
    await page.getByRole("button", { name: "Copy Chinese" }).click();
    expect(await clipboard(page)).toBe(sources.join("\n\n"));

    const results = page.getByTestId("results");
    await expect(results).toBeVisible();
    await page.getByRole("button", { name: "Show original" }).hover();
    await page.mouse.down();
    await expect(results).toBeHidden();
    await page.mouse.up();
    await expect(results).toBeVisible();
    await page.keyboard.down("Space");
    await expect(results).toBeHidden();
    await page.keyboard.up("Space");
    await expect(results).toBeVisible();

    await page.getByRole("button", { name: "Close" }).click();
    expect(await calls(page, "screen_close")).toHaveLength(1);
  });

  test("offers the missing text recognition model, shows its progress and reads the area again", async ({ page }) => {
    await start(page, { models: [OCR_MODEL] });
    await drag(page, AREA.from, AREA.to);
    await send(page, [
      { type: "reading", job: 1 },
      { type: "error", job: 1, message: "Cannot open the OCR model: file not found" },
    ]);
    await expect(page.getByRole("status")).toContainText("Screen translation needs the text recognition model (23 MB).");

    await page.getByRole("button", { name: "Download" }).click();
    expect(await calls(page, "models_download_optional")).toEqual([
      { cmd: "models_download_optional", args: { id: OCR_MODEL.id } },
    ]);
    await emitNamed(page, "models://progress", [{ ...OCR_MODEL, state: "downloading", bytes_done: 12_000_000 }]);
    await expect(page.getByRole("progressbar")).toHaveAttribute("value", "50");
    await expect(page.getByRole("status")).toContainText("Downloading the text recognition model… 50%");

    await emitNamed(page, "models://progress", [{ ...OCR_MODEL, state: "ready", bytes_done: OCR_MODEL.bytes_total }]);
    await expect.poll(async () => (await calls(page, "screen_select")).length).toBe(2);
    await expect(page.getByRole("status")).toHaveText("Reading text…");
  });
});
