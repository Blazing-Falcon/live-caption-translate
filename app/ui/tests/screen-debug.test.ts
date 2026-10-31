import { expect, it } from "vitest";
import { GROUP_OUTSET, groupBoxes, lineBoxes } from "../src/lib/screen-debug";
import type { Rect, ScreenBlock } from "../src/lib/types";

function block(source: string, rect: Rect, lines = [rect]): ScreenBlock {
  const float = (r: Rect) => ({ x: r.x, y: r.y, width: r.w, height: r.h });
  return { id: 0, source, rect, lines, line_height: rect.h, intent: "single", background: [255, 255, 255], foreground: [0, 0, 0], translate: false,
    paint: { bounds: float(rect), source_lines: lines.map(float), render_glyph_height: null, font_glyph_height: null } };
}

it("draws a box for every recognized line", () => {
  const blocks = [block("a wrapped sentence", { x: 10, y: 10, w: 200, h: 60 }, [{ x: 10, y: 10, w: 200, h: 28 }, { x: 10, y: 42, w: 180, h: 28 }]), block("Exit", { x: 10, y: 120, w: 70, h: 28 })];
  expect(lineBoxes(blocks)).toHaveLength(3);
  for (const rect of [{ x: 10, y: 10, w: 200, h: 28 }, { x: 10, y: 42, w: 180, h: 28 }, { x: 10, y: 120, w: 70, h: 28 }]) expect(lineBoxes(blocks)).toContainEqual(rect);
});

it("places a group box outside the lines it holds", () => {
  const blocks = [block("Exit", { x: 10, y: 120, w: 70, h: 28 })];
  const groups = groupBoxes(blocks), lines = lineBoxes(blocks);
  expect(groups).toHaveLength(1); expect(lines).toHaveLength(1);
  const group = groups[0]!, line = lines[0]!;
  expect(group.x).toBeLessThanOrEqual(line.x); expect(group.y).toBeLessThanOrEqual(line.y);
  expect(group.x + group.width).toBeGreaterThanOrEqual(line.x + line.w);
  expect(group.y + group.height).toBeGreaterThanOrEqual(line.y + line.h);
  expect(group.x).toBe(line.x - GROUP_OUTSET);
  expect(group.x + group.width).toBe(line.x + line.w + GROUP_OUTSET);
});
