import { readFileSync, readdirSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import type { Rect, RectF, ScreenBlock, ScreenDirection, ScreenEvent, ScreenFrame } from "../../src/lib/types";
import { calls, emitNamed, open } from "./support";

interface Fixture {
  name: string; direction: ScreenDirection; frame: ScreenFrame; selection: Rect;
  blocks: ScreenBlock[]; translations: string[];
  expected: { id: number; bubbleRect: RectF; textRect: RectF; fontSize: number; wrappedLineCount: number }[];
}
const folder = new URL("../fixtures/directions/", import.meta.url);
const fixtures = readdirSync(folder).filter(name => name.endsWith(".json") && !name.endsWith(".screen.json"))
  .map(name => JSON.parse(readFileSync(new URL(name, folder), "utf8")) as Fixture);

async function drag(page: Page, rect: Rect): Promise<void> {
  await page.mouse.move(rect.x, rect.y); await page.mouse.down();
  await page.mouse.move(rect.x + rect.w, rect.y + rect.h, { steps: 3 }); await page.mouse.up();
}
async function frame(page: Page, value: ScreenFrame, direction: ScreenDirection): Promise<void> {
  await page.setViewportSize({ width: value.width / value.scale, height: value.height / value.scale });
  await page.route("http://ltshot.localhost/**", route => route.fulfill({ contentType: "image/svg+xml", body: `<svg xmlns="http://www.w3.org/2000/svg" width="${value.width}" height="${value.height}"/>` }));
  await open(page, "screen", { config: { screen: { direction } } });
  await emitNamed(page, "screen://frame", value);
  await expect(page.getByLabel("Text direction")).toHaveValue(direction);
}
async function show(page: Page, blocks: ScreenBlock[], texts: string[]): Promise<void> {
  const tilted = blocks.filter(block => block.tilt).map(block => block.id);
  await page.evaluate(ids => {
    const reply = window.__lt.backend.plateReply;
    window.__lt.backend.plateReply = async request => ({ ...await reply(request), transform_with_block: ids.includes(request.block) });
  }, tilted);
  await emitNamed(page, "screen://event", { type: "reading", job: 1 });
  await emitNamed(page, "screen://event", { type: "blocks", job: 1, blocks, ocr_ms: 0 });
  for (let i = 0; i < blocks.length; i++) await emitNamed(page, "screen://event", { type: "translated", job: 1, id: blocks[i]!.id, text: texts[i] });
  await emitNamed(page, "screen://event", { type: "done", job: 1, total_ms: 0 });
  await expect(page.locator("[data-plate]")).toHaveCount(blocks.length);
  await expect.poll(() => page.locator("[data-block]").evaluateAll(nodes => nodes.every(node => getComputedStyle(node).backgroundColor === "rgba(0, 0, 0, 0)"))).toBe(true);
}
for (const fixture of fixtures) {
  test(`vertical and tilted geometry matches native layout: ${fixture.name}`, async ({ page }) => {
    await frame(page, fixture.frame, fixture.direction);
    await drag(page, fixture.selection);
    await show(page, fixture.blocks, fixture.translations);
    for (const expected of fixture.expected) {
      const block = fixture.blocks.find(block => block.id === expected.id)!;
      const container = page.locator(`[data-block="${expected.id}"]`), written = container.locator(".written");
      const textBox = await written.boundingBox(), bubble = await (block.intent === "across_in_columns" && !block.tilt?.column ? written : container).boundingBox();
      expect(textBox).not.toBeNull(); expect(bubble).not.toBeNull();
      for (const key of ["x", "y", "width", "height"] as const) {
        expect(Math.abs(textBox![key] - expected.textRect[key]), `${expected.id} text ${key}`).toBeLessThanOrEqual(1);
        expect(Math.abs(bubble![key] - expected.bubbleRect[key]), `${expected.id} container ${key}`).toBeLessThanOrEqual(1);
      }
      const metrics = await container.evaluate(node => ({ font: parseFloat(getComputedStyle(node).fontSize), lines: Number(node.getAttribute("data-lines")) }));
      expect(Math.abs(metrics.font - expected.fontSize)).toBeLessThanOrEqual(0.5);
      expect(metrics.lines).toBe(expected.wrappedLineCount);
      if (block.tilt) {
        const plate = page.locator(`[data-plate="${expected.id}"]`);
        expect(await plate.evaluate(node => getComputedStyle(node).transform)).toBe(await container.evaluate(node => getComputedStyle(node).transform));
        const imageBox = await plate.boundingBox();
        for (const key of ["x", "y", "width", "height"] as const) expect(Math.abs(imageBox![key] - (await container.boundingBox())![key])).toBeLessThanOrEqual(0.02);
      }
    }
  });
}

test("the English screenshot bubble covers its source column and its entire translation", async ({ page }) => {
  await frame(page, { id: 7, width: 941, height: 900, scale: 1 }, "vertical");
  await drag(page, { x: 0, y: 0, w: 941, h: 900 });
  const bounds = { x: 400, y: 40, width: 34, height: 300 };
  const block: ScreenBlock = { id: 0, rect: { x: 400, y: 40, w: 34, h: 300 }, lines: [], intent: "across_in_columns", line_height: 34,
    source: "これはたてがき", background: [255, 255, 255], foreground: [0, 0, 0], translate: true,
    paint: { bounds, source_lines: [bounds], render_glyph_height: 34, font_glyph_height: null } };
  const text = "I really did not think you would actually come back here";
  await show(page, [block], [text]);
  const bubble = (await page.locator("[data-block]").boundingBox())!, written = (await page.locator(".written").boundingBox())!;
  expect(await page.locator(".written").textContent()).toBe(text);
  expect(written.height).toBeGreaterThan(0);
  for (const box of [bounds, written]) {
    expect(bubble.x).toBeLessThanOrEqual(box.x + 0.5); expect(bubble.y).toBeLessThanOrEqual(box.y + 0.5);
    expect(bubble.x + bubble.width).toBeGreaterThanOrEqual(box.x + box.width - 0.5);
    expect(bubble.y + bubble.height).toBeGreaterThanOrEqual(box.y + box.height - 0.5);
  }
});

test("a recorded vertical job keeps complete English at 150 percent and rereads when direction changes", async ({ page }) => {
  const recording = JSON.parse(readFileSync(new URL("recorded-vertical.screen.json", folder), "utf8")) as { width: number; height: number; direction: string; events: { event: ScreenEvent }[] };
  expect(recording.direction).toBe("vertical");
  const value = { id: 7, width: recording.width, height: recording.height, scale: 1.5 };
  await frame(page, value, "vertical");
  await drag(page, { x: 0, y: 0, w: value.width / value.scale, h: value.height / value.scale });
  await expect.poll(async () => (await calls(page, "screen_select")).length).toBe(1);
  for (const { event } of recording.events) await emitNamed(page, "screen://event", event);
  const translated = recording.events.map(row => row.event).filter((event): event is Extract<ScreenEvent, { type: "translated" }> => event.type === "translated");
  await expect(page.locator("[data-block]")).toHaveCount(translated.length);
  for (const event of translated) {
    const node = page.locator(`[data-block="${event.id}"]`);
    await expect(node.locator(".written")).toHaveText(event.text);
    expect(await node.evaluate(element => getComputedStyle(element).overflow)).toBe("visible");
  }
  const previous = (await calls(page, "screen_select"))[0]!.args;
  expect(previous.direction).toBe("vertical");
  await page.getByLabel("Text direction").selectOption("horizontal");
  await expect.poll(async () => (await calls(page, "screen_select")).length).toBe(2);
  expect((await calls(page, "screen_select"))[1]!.args).toEqual({ ...previous, direction: "horizontal" });
  expect(await page.evaluate(() => window.__lt.backend.config.screen.direction)).toBe("horizontal");
});
