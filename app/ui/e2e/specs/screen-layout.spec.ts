import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import type { Rect, RectF, ScreenBlock, ScreenFrame } from "../../src/lib/types";
import { calls, emitNamed, open } from "./support";

interface Expected {
  id: number;
  bubbleRect: RectF;
  fontSize: number;
  wrappedLineCount: number;
}
interface Fixture {
  name: string;
  frame: ScreenFrame;
  selection: Rect;
  blocks: ScreenBlock[];
  translations: string[];
  expected: Expected[];
}
const folder = new URL("../fixtures/layout/", import.meta.url);
const accepted = JSON.parse(readFileSync(new URL("accepted-renderer.json", folder), "utf8")) as {
  case: string; id: number;
  layout: { bubble: RectF; fontSize: number; wrappedLineCount: number; writtenHeight: number };
  original: Expected;
};
const fixtures = readdirSync(folder).filter(name => name.endsWith(".json") && name !== "accepted-renderer.json")
  .map(name => JSON.parse(readFileSync(new URL(name, folder), "utf8")) as Fixture);

async function select(page: Page, rect: Rect): Promise<void> {
  await page.mouse.move(rect.x, rect.y);
  await page.mouse.down();
  await page.mouse.move(rect.x + rect.w, rect.y + rect.h, { steps: 3 });
  await page.mouse.up();
}

async function render(page: Page, fixture: Fixture): Promise<void> {
  await page.setViewportSize({ width: fixture.frame.width / fixture.frame.scale, height: fixture.frame.height / fixture.frame.scale });
  await page.route("http://ltshot.localhost/**", route => route.fulfill({
    contentType: "image/svg+xml", body: `<svg xmlns="http://www.w3.org/2000/svg" width="${fixture.frame.width}" height="${fixture.frame.height}"/>`,
  }));
  await open(page, "screen");
  await emitNamed(page, "screen://frame", fixture.frame);
  await expect.poll(() => page.locator(".frame").evaluate(node => (node as HTMLImageElement).naturalWidth)).toBe(fixture.frame.width);
  await select(page, { x: fixture.selection.x / fixture.frame.scale, y: fixture.selection.y / fixture.frame.scale, w: fixture.selection.w / fixture.frame.scale, h: fixture.selection.h / fixture.frame.scale });
  await emitNamed(page, "screen://event", { type: "reading", job: 1 });
  await emitNamed(page, "screen://event", { type: "blocks", job: 1, blocks: fixture.blocks, ocr_ms: 0 });
  for (let i = 0; i < fixture.blocks.length; i++) {
    await emitNamed(page, "screen://event", { type: "translated", job: 1, id: fixture.blocks[i]!.id, text: fixture.translations[i] });
  }
  await emitNamed(page, "screen://event", { type: "done", job: 1, total_ms: 0 });
  await expect.poll(async () => (await calls(page, "screen_plate")).length).toBe(fixture.blocks.length);
  await expect(page.locator("[data-plate]")).toHaveCount(fixture.blocks.length);
  await expect.poll(() => page.locator("[data-block]").evaluateAll(nodes => nodes.every(node => getComputedStyle(node).backgroundColor === "rgba(0, 0, 0, 0)"))).toBe(true);
}

for (const fixture of fixtures) {
  test(`horizontal geometry and whole English match recorded layout: ${fixture.name}`, async ({ page }, testInfo) => {
    await render(page, fixture);
    const requests = await calls(page, "screen_plate");
    for (const original of fixture.expected) {
      const element = page.locator(`[data-block="${original.id}"]`);
      const metrics = await element.evaluate(node => {
        const style = getComputedStyle(node), box = node.getBoundingClientRect();
        return { rect: { x: box.x, y: box.y, width: box.width, height: box.height }, font: parseFloat(style.fontSize),
          lines: Number(node.getAttribute("data-lines")), source: Number(node.getAttribute("data-source-font")) };
      });
      const request = requests.find(call => (call.args.request as { block: number }).block === original.id)?.args.request as { rect: RectF; written_height: number };
      const exception = fixture.name === accepted.case && original.id === accepted.id;
      const expected = exception ? { bubbleRect: accepted.layout.bubble, fontSize: accepted.layout.fontSize, wrappedLineCount: accepted.layout.wrappedLineCount } : original;
      for (const key of ["x", "y", "width", "height"] as const) {
        expect(Math.abs(request.rect[key] - expected.bubbleRect[key]), `${original.id} request ${key}`).toBeLessThanOrEqual(1);
        expect(Math.abs(metrics.rect[key] - expected.bubbleRect[key]), `${original.id} drawn ${key}`).toBeLessThanOrEqual(1);
      }
      expect(Math.abs(metrics.font - expected.fontSize), `${original.id} font`).toBeLessThanOrEqual(0.5);
      expect(metrics.lines, `${original.id} lines`).toBe(expected.wrappedLineCount);
      expect(metrics.font).toBeLessThanOrEqual(metrics.source + 0.001);
      if (exception) {
        for (const key of ["x", "y", "width", "height"] as const) expect(request.rect[key]).toBeCloseTo(accepted.layout.bubble[key], 8);
        expect(metrics.font).toBeCloseTo(accepted.layout.fontSize, 3);
        expect(request.written_height).toBeCloseTo(accepted.layout.writtenHeight, 8);
        expect(Math.abs(metrics.font - accepted.original.fontSize)).toBeGreaterThan(0.5);
      }
    }
    await expect.poll(() => page.locator("[data-plate]").evaluateAll(nodes => nodes.every(node => (node as HTMLImageElement).complete))).toBe(true);
    await page.locator(".bar").evaluate(node => { (node as HTMLElement).style.visibility = "hidden"; });
    const clipped = await page.screenshot();
    await page.locator(".box.modern").evaluateAll(nodes => nodes.forEach(node => { (node as HTMLElement).style.overflow = "visible"; }));
    const whole = await page.screenshot();
    if (!clipped.equals(whole)) {
      writeFileSync(testInfo.outputPath("clipped.png"), clipped);
      writeFileSync(testInfo.outputPath("whole.png"), whole);
      await testInfo.attach("clipped", { body: clipped, contentType: "image/png" });
      await testInfo.attach("whole", { body: whole, contentType: "image/png" });
    }
    expect(clipped.equals(whole), "all glyph pixels remain visible with clipping enabled").toBe(true);
  });
}

test("measures unbreakable English and explicit newlines as actually drawn", async ({ page }) => {
  const source = fixtures.find(f => f.name === "article@1")!;
  const make = (id: number, y: number): ScreenBlock => ({ ...source.blocks[0]!, id, source: "中文文字", rect: { x: 30, y, w: 70, h: 24 }, lines: [{ x: 30, y, w: 70, h: 24 }], intent: "single", paint: {
    bounds: { x: 30, y, width: 70, height: 24 }, source_lines: [{ x: 30, y, width: 70, height: 24 }], render_glyph_height: 24, font_glyph_height: null,
  } });
  const fixture: Fixture = { name: "wrapping", frame: { id: 7, width: 300, height: 500, scale: 1 }, selection: { x: 0, y: 0, w: 110, h: 500 },
    blocks: [make(0, 25), make(1, 280)], translations: ["Supercalifragilisticexpialidocious and a very long description of the event.", "Hello there\nHow are you today?"], expected: [] };
  await render(page, fixture);
  const metrics = await page.locator("[data-block]").evaluateAll(nodes => nodes.map(node => {
    const box = node.getBoundingClientRect(), text = node.querySelector(".written")!.getBoundingClientRect();
    return { top: box.top, bottom: box.bottom, textTop: text.top, textBottom: text.bottom, fullText: node.textContent };
  }));
  expect(metrics).toHaveLength(2);
  expect(metrics[0]!.fullText).toBe(fixture.translations[0]);
  expect(metrics[1]!.fullText).toBe(fixture.translations[1]);
  for (const metric of metrics) expect(metric.textBottom).toBeLessThanOrEqual(metric.bottom + 0.5);
  expect(metrics[0]!.bottom).toBeLessThanOrEqual(metrics[1]!.textTop);
});

test.describe("fractional display scale", () => {
  test.use({ deviceScaleFactor: 1.5 });
  test("keeps the same CSS layout and sends physical plate geometry at 150 percent", async ({ page }) => {
    const base = fixtures.find(f => f.name === "article@1")!, scale = 1.5;
    const float = (r: RectF): RectF => ({ x: r.x * scale, y: r.y * scale, width: r.width * scale, height: r.height * scale });
    const integer = (r: Rect): Rect => ({ x: r.x * scale, y: r.y * scale, w: r.w * scale, h: r.h * scale });
    const fixture: Fixture = { ...base, frame: { ...base.frame, width: base.frame.width * scale, height: base.frame.height * scale, scale },
      selection: integer(base.selection), blocks: base.blocks.map(b => ({ ...b, rect: integer(b.rect), lines: b.lines.map(integer), line_height: b.line_height * scale,
        paint: { ...b.paint!, bounds: float(b.paint!.bounds), source_lines: b.paint!.source_lines.map(float),
          render_glyph_height: b.paint!.render_glyph_height === null ? null : b.paint!.render_glyph_height * scale,
          font_glyph_height: b.paint!.font_glyph_height === null ? null : b.paint!.font_glyph_height * scale } })) };
    await render(page, fixture);
    const requests = await calls(page, "screen_plate");
    for (const expected of base.expected) {
      const node = page.locator(`[data-block="${expected.id}"]`);
      const box = await node.boundingBox();
      expect(box!.x).toBeCloseTo(expected.bubbleRect.x, 0);
      expect(box!.y).toBeCloseTo(expected.bubbleRect.y, 0);
      expect(box!.width).toBeCloseTo(expected.bubbleRect.width, 0);
      expect(box!.height).toBeCloseTo(expected.bubbleRect.height, 0);
      const font = await node.evaluate(n => parseFloat(getComputedStyle(n).fontSize));
      expect(Math.abs(font - expected.fontSize)).toBeLessThanOrEqual(0.5);
      const request = requests.find(call => (call.args.request as { block: number }).block === expected.id)?.args.request as { rect: RectF };
      for (const key of ["x", "y", "width", "height"] as const) expect(Math.abs(request.rect[key] - expected.bubbleRect[key] * scale)).toBeLessThanOrEqual(1);
    }
    await page.locator(".bar").evaluate(n => { (n as HTMLElement).style.visibility = "hidden"; });
    const clipped = await page.screenshot();
    await page.locator(".box.modern").evaluateAll(nodes => nodes.forEach(n => { (n as HTMLElement).style.overflow = "visible"; }));
    expect(clipped.equals(await page.screenshot())).toBe(true);
  });
});
