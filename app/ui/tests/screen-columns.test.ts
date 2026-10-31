import { describe, expect, it } from "vitest";
import { acrossInColumnsFit as fit, acrossInColumnsRoom as room, type Measurer } from "../src/lib/screen-layout";

const mono: Measurer = { measure(text, size, width) {
  const advance = size * 0.55, lineHeight = size * 1.3;
  if (width === undefined) return { width: text.length * advance, height: lineHeight };
  const perLine = Math.max(1, Math.floor(width / advance + 1e-9));
  let lines = 0, used = 0, widest = 0;
  for (const word of text.split(" ").filter(Boolean)) {
    const need = used === 0 ? word.length : used + 1 + word.length;
    if (used > 0 && need <= perLine) used = need;
    else {
      if (used > 0) widest = Math.max(widest, used * advance);
      const pieces = Math.floor((word.length + perLine - 1) / perLine);
      lines += pieces;
      used = word.length - (pieces - 1) * perLine;
      if (pieces > 1) widest = Math.max(widest, perLine * advance);
    }
  }
  return { width: Math.max(widest, used * advance), height: Math.max(1, lines) * lineHeight };
} };
const sentence = "I really did not think you would actually come back here";
const none = { left: 0, right: 0, below: 0 };

describe("English in columns", () => {
  it("fits a sentence to a one-column balloon with whole words", () => {
    const result = fit(sentence, 30, 300, 26, 9.5, 7, { left: 40, right: 40, below: 0 }, mono);
    expect(result.width).toBeGreaterThanOrEqual(Math.max(...sentence.split(" ").map(word => mono.measure(word, result.fontSize).width)));
    expect(result.height).toBeLessThanOrEqual(300.01);
    expect(result.left).toBeGreaterThanOrEqual(-40.01);
    expect(result.left + result.width).toBeLessThanOrEqual(70.01);
    expect(result.fontSize).toBeLessThanOrEqual(26);
  });
  it("centres a short translation at its preferred size", () => {
    const result = fit("What?", 30, 300, 24, 9.5, 7, { left: 200, right: 200, below: 200 }, mono);
    expect(result.fontSize).toBe(24);
    expect(result.left + result.width / 2).toBeCloseTo(15, 1);
    expect(result.top + result.height / 2).toBeCloseTo(150, 1);
  });
  it("fills the balloon shape instead of a wide band", () => {
    expect(fit(sentence, 30, 300, 14, 9.5, 7, { left: 400, right: 400, below: 0 }, mono).width).toBeLessThan(58.01);
  });
  it("keeps a long compound whole when any readable size can hold it", () => {
    const word = "Donaudampfschifffahrtsgesellschaft";
    const roomy = fit(word, 30, 300, 20, 9.5, 7, { left: 150, right: 150, below: 0 }, mono);
    expect(roomy.width).toBeGreaterThanOrEqual(mono.measure(word, roomy.fontSize).width);
    const tight = fit(word, 30, 300, 20, 9.5, 7, { left: 60, right: 60, below: 0 }, mono);
    expect(tight.fontSize).toBeGreaterThanOrEqual(9.5);
    expect(tight.width).toBeLessThanOrEqual(150.01);
    expect(tight.height).toBeLessThanOrEqual(300.01);
  });
  it("keeps the entire sentence at emergency size when nothing fits", () => {
    const text = "words that will never fit ".repeat(40);
    const result = fit(text, 30, 60, 20, 9.5, 7, none, mono);
    expect(result.fontSize).toBe(7);
    expect(result.height).toBeCloseTo(mono.measure(text, 7, result.width).height, 3);
    expect(result.top).toBe(0);
  });
  it("takes available room before giving up a fixed size", () => {
    const available = { left: 200, right: 200, below: 0 };
    expect(fit(sentence, 30, 120, 24, 9.5, 7, available, mono).fontSize).toBeLessThan(24);
    expect(fit(sentence, 30, 120, 24, 9.5, 7, available, mono, true).fontSize).toBe(24);
  });
  it("grows down at fixed size and shrinks only to keep a word whole", () => {
    const result = fit(sentence, 120, 80, 24, 9.5, 7, { left: 0, right: 0, below: 400 }, mono, true);
    expect(result.fontSize).toBe(24);
    expect(result.height).toBeGreaterThan(80);
    const word = fit("preparing for the next exploration.", 100, 300, 24, 9.5, 7, none, mono, true);
    expect(word.fontSize).toBeLessThan(24);
    expect(mono.measure("exploration.", word.fontSize).width).toBeLessThanOrEqual(word.width);
  });
  it("closes the overlapping sides of two blocks in one balloon", () => {
    const upper = { x: 711, y: 573.5, width: 83, height: 206 };
    const lower = { x: 637.5, y: 682, width: 80.5, height: 204 };
    const page = { x: 0, y: 0, width: 827, height: 1276 };
    expect(room(upper, [lower], page)).toEqual({ left: 0, right: 33, below: 0 });
    expect(room(lower, [upper], page)).toEqual({ left: 637.5, right: 0, below: 390 });
  });
  it("breaks an oversized word once before breaking it more", () => {
    const word = "Donaudampfschifffahrtsgesellschaft!";
    const result = fit(word, 60, 300, 26, 9.5, 7, { left: 20, right: 20, below: 0 }, mono);
    const whole = mono.measure(word, result.fontSize).width;
    expect(whole).toBeGreaterThan(result.width);
    expect(whole).toBeLessThanOrEqual(result.width * 2 + 0.01);
    expect(result.fontSize).toBeGreaterThanOrEqual(9.5);
  });
  it("shrinks an ordinary word whole before breaking it", () => {
    const result = fit("Yesterday's thoughts still linger in my mind.", 52, 400, 16, 9.5, 7, none, mono);
    expect(result.fontSize).toBeLessThan(9.5);
    expect(mono.measure("Yesterday's", result.fontSize).width).toBeLessThanOrEqual(result.width);
  });
  it("takes half a neighbour gap and the whole page-edge gap", () => {
    expect(room({ x: 100, y: 50, width: 30, height: 200 }, [
      { x: 40, y: 60, width: 30, height: 100 }, { x: 100, y: 300, width: 30, height: 50 }, { x: 500, y: 600, width: 10, height: 10 },
    ], { x: 0, y: 0, width: 200, height: 400 })).toEqual({ left: 15, right: 70, below: 50 });
  });
});
