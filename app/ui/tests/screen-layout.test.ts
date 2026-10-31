import { describe, expect, it } from "vitest";
import { sourceFontSize, singleLineLayout, wrappedBubbleHeight, horizontalLayout, type Measurer } from "../src/lib/screen-layout";
import type { ScreenBlock } from "../src/lib/types";

describe("source font scale", () => {
  it.each([false, true])("neighbouring source heights produce neighbouring font sizes (%s)", (latin) => {
    for (let height = 8; height <= 30; height += 0.1) expect(sourceFontSize(height + 0.1, latin) - sourceFontSize(height, latin)).toBeLessThanOrEqual(0.25);
  });
  it.each([false, true])("larger source text never produces a smaller font (%s)", (latin) => {
    for (let height = 8; height <= 30; height += 0.1) expect(sourceFontSize(height + 0.1, latin)).toBeGreaterThanOrEqual(sourceFontSize(height, latin) - 1e-9);
  });
  it.each([[10, 14.5], [12, 14.5], [20, 21.2], [28, 29.68]])("keeps the ratios outside the blend band (%s)", (height, expected) => {
    expect(sourceFontSize(height!)).toBeCloseTo(expected!, 2);
  });
  it("boosts small Latin rendered as CJK and leaves large text unchanged", () => {
    expect(sourceFontSize(12, true)).toBeCloseTo(12 * 1.18 * 1.08, 2);
    expect(sourceFontSize(12, true)).toBeGreaterThan(sourceFontSize(12, false));
    expect(sourceFontSize(20, true)).toBeCloseTo(sourceFontSize(20, false), 2);
  });
  it("barely changes for half a pixel across the old threshold", () => {
    const before = sourceFontSize(14.1, true), after = sourceFontSize(13.6, true);
    expect(Math.abs(before - after) / before).toBeLessThan(0.05);
  });
});

describe("single line layout", () => {
  const calculate = (textWidth: number, rightExpansionWidth: number, maxExpandedWidth = 300) => singleLineLayout({
    originalFontSize: 16, originalBorderWidth: 106, textWidth, horizontalPadding: 6,
    rightExpansionWidth, maxExpandedWidth, readableMinFontSize: 10, absoluteMinFontSize: 9.5,
  });
  it("keeps the original font and expands right when room allows", () => {
    expect(calculate(120, 130)).toEqual({ fontSize: 16, borderWidth: 126, preferRightExpansion: true });
  });
  it("does not enlarge a font when the translation is shorter", () => {
    const result = calculate(40, 80);
    expect(result.fontSize).toBeCloseTo(16, 3); expect(result.borderWidth).toBeCloseTo(106, 3); expect(result.preferRightExpansion).toBe(false);
  });
  it("shrinks within the original width before expanding both sides", () => {
    const result = calculate(160, 80);
    expect(result.fontSize).toBeCloseTo(10, 3); expect(result.borderWidth).toBeCloseTo(106, 3); expect(result.preferRightExpansion).toBe(false);
  });
  it("expands both sides when the original width falls below the readable floor", () => {
    const result = calculate(220, 80);
    expect(result.fontSize).toBeCloseTo(10, 3); expect(result.borderWidth).toBeCloseTo(143.5, 3); expect(result.preferRightExpansion).toBe(false);
  });
  it("uses the absolute floor when the expanded width is insufficient", () => {
    const result = calculate(400, 80, 140);
    expect(result.fontSize).toBeCloseTo(9.5, 3); expect(result.borderWidth).toBe(140); expect(result.preferRightExpansion).toBe(false);
  });
});

describe("wrapped bubble height", () => {
  it("grows to the text with nothing below", () => expect(wrappedBubbleHeight(30, 30, 74, null)).toBe(74));
  it("respects a gap that holds the text", () => expect(wrappedBubbleHeight(30, 30, 74, 90)).toBe(74));
  it("keeps the text when the gap is too small", () => expect(wrappedBubbleHeight(30, 30, 74, 40)).toBe(74));
  it("never uncovers its source", () => expect(wrappedBubbleHeight(30, 30, 24, 18)).toBe(30));
  it("leaves unused room alone", () => expect(wrappedBubbleHeight(30, 30, 22, 400)).toBe(30));
});

it("keeps the source font floor in display units at 150 percent scale", () => {
  expect(sourceFontSize(18, false, 1.5)).toBe(21.75);
});

it("scales grouped geometry, fitting steps and padding together", () => {
  const make = (scale: number): ScreenBlock => ({
    id: 0, rect: { x: 20 * scale, y: 30 * scale, w: 160 * scale, h: 40 * scale }, lines: [], intent: "group_reflow",
    line_height: 20 * scale, source: "中文\n文字", background: [255, 255, 255], foreground: [0, 0, 0], translate: true,
    paint: { bounds: { x: 20 * scale, y: 30 * scale, width: 160 * scale, height: 40 * scale },
      source_lines: [30, 50].map(y => ({ x: 20 * scale, y: y * scale, width: 160 * scale, height: 20 * scale })),
      render_glyph_height: 18 * scale, font_glyph_height: null },
  });
  const measurer: Measurer = { measure(text, size, width) {
    const natural = text.length * size / 2;
    return { width: width === undefined ? natural : Math.min(natural, width), height: size * 1.33 * (width === undefined ? 1 : Math.max(1, Math.ceil(natural / width))) };
  } };
  const result = (scale: number) => {
    const block = make(scale);
    return horizontalLayout(block, "An extended English sentence with several words to fit inside two rows.", [block], {
      width: 600 * scale, height: 600 * scale, scale, selection: { x: 0, y: 0, width: 600 * scale, height: 600 * scale },
    }, measurer);
  };
  const original = result(1), scaled = result(1.5);
  expect(scaled.wrappedLineCount).toBe(original.wrappedLineCount);
  expect(scaled.fontSize).toBeCloseTo(original.fontSize * 1.5, 8);
  for (const key of ["x", "y", "width", "height"] as const) expect(scaled.bubble[key]).toBeCloseTo(original.bubble[key] * 1.5, 8);
});
