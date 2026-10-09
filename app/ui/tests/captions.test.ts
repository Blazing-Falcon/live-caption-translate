import { get } from "svelte/store";
import { describe, expect, it, vi } from "vitest";
import {
  CHIP_MS,
  FADE_MS,
  MAX_HISTORY_LINES,
  MAX_OUTSTANDING_LINES,
  ageOpacity,
  createCaptionController,
  failReasonText,
  visibleLines,
  type CaptionController,
} from "../src/lib/captions";
import type { CaptionLine, FailReason, PipelineEvent } from "../src/lib/types";
import {
  FakeClock,
  STATS,
  asrFinal,
  delta,
  failed,
  final,
  joined,
  skipped,
  testConfig,
  testState,
} from "./helpers";

function setup(options: { reducedMotion?: boolean; configure?: Parameters<typeof testConfig>[0] } = {}) {
  const clock = new FakeClock();
  const warn = vi.fn();
  const controller = createCaptionController({ clock, reducedMotion: () => options.reducedMotion ?? false, onWarn: warn });
  controller.setConfig(testConfig(options.configure));
  const view = () => get(controller);
  const lines = () => view().lines;
  const run = (...events: PipelineEvent[]) => events.forEach((event) => controller.dispatch(event));
  return { clock, warn, controller, view, lines, run };
}

const summary = (lines: readonly CaptionLine[]) => lines.map((line) => `${line.id}:${line.state}`);

describe("caption reducer", () => {
  it("walks pending -> streaming -> final and keeps the source", () => {
    const { lines, run, clock } = setup();
    run(asrFinal(1, "你好。"));
    expect(lines()).toMatchObject([{ id: 1, state: "pending", source: "你好。", english: "", lang: "zh", reason: null }]);
    clock.advance(500);
    run(delta(1, "Hel"));
    expect(lines()).toMatchObject([{ state: "streaming", english: "Hel", source: "你好。", updatedAt: 500 }]);
    run(final(1, "Hello."));
    expect(lines()).toMatchObject([{ state: "final", english: "Hello." }]);
  });

  it("joins absorbed ids into the leader, which keeps the earliest position", () => {
    const { lines, run } = setup();
    run(asrFinal(10, "我也想"), asrFinal(11, "办一个"), asrFinal(12, "公司"), asrFinal(13, "别的话"));
    run(joined(10, [11], "我也想，办一个", "hold"));
    expect(summary(lines())).toEqual(["10:pending", "12:pending", "13:pending"]);
    expect(lines()[0]?.source).toBe("我也想，办一个");
    run(joined(10, [12], "我也想，办一个，公司", "queue"));
    expect(summary(lines())).toEqual(["10:pending", "13:pending"]);
    expect(lines()[0]?.source).toBe("我也想，办一个，公司");
    run(delta(10, "I also"), final(10, "I also want to run a company."));
    expect(lines()[0]).toMatchObject({ state: "final", source: "我也想，办一个，公司" });
  });

  it("ignores duplicate asr_final and anything after a terminal event", () => {
    const { lines, run } = setup();
    run(asrFinal(31, "你好"), final(31, "Hello"), asrFinal(31, "你好"), delta(31, "Hel"), failed(31, "timeout"), skipped(31));
    expect(lines()).toMatchObject([{ state: "final", english: "Hello" }]);
  });

  it.each<[FailReason, string, string]>([
    ["timeout", "ignored", "Translation took too long"],
    ["server_unavailable", "ignored", "Translator is restarting"],
    ["echo", "ignored", "Could not translate this line"],
    ["runaway", "ignored", "Could not translate this line"],
    ["error", "model exploded", "model exploded"],
    ["error", "   ", "Could not translate this line"],
  ])("maps failure %s to a user-facing reason", (reason, message, expected) => {
    const { lines, run } = setup();
    run(asrFinal(60, "这件事说来话长。"), delta(60, "It is"), failed(60, reason, message));
    expect(lines()[0]).toMatchObject({ state: "failed", reason: expected, source: "这件事说来话长。" });
    expect(failReasonText(reason, message)).toBe(expected);
  });

});

describe("expiry", () => {
  const settle = (c: ReturnType<typeof setup>, id: number) => {
    c.run(asrFinal(id, `行${id}`), final(id, `Line ${id}`));
  };

  it("never expires pending or streaming rows, and expires them after they settle", () => {
    const c = setup();
    c.run(asrFinal(1, "一"), asrFinal(2, "二"), asrFinal(3, "三"), final(1, "One"), delta(2, "Tw"));
    c.clock.advance(8000 + FADE_MS);
    expect(summary(c.lines())).toEqual(["2:streaming", "3:pending"]);
    c.clock.advance(60_000);
    expect(summary(c.lines())).toEqual(["2:streaming", "3:pending"]);
    c.run(final(2, "Two"));
    c.clock.advance(0);
    expect(c.view().fadingIds.has(2)).toBe(true);
    expect(c.view().fadingIds.has(3)).toBe(false);
    c.clock.advance(FADE_MS);
    expect(summary(c.lines())).toEqual(["3:pending"]);
    c.run(skipped(3));
    c.clock.advance(0);
    c.clock.advance(FADE_MS);
    expect(c.lines()).toHaveLength(0);
  });

  it("bounds history and outstanding rows during an hour of continuous speech", () => {
    const c = setup();
    let peak = 0;
    for (let id = 1; id <= 900; id += 1) {
      c.run(asrFinal(id, `句子${id}`));
      c.clock.advance(1500);
      c.run(delta(id, "So"), final(id, "Some text."));
      c.clock.advance(2500);
      peak = Math.max(peak, c.lines().length);
      expect(c.lines().length).toBeLessThanOrEqual(MAX_HISTORY_LINES + 1);
    }
    expect(peak).toBeLessThanOrEqual(MAX_HISTORY_LINES + 1);
    expect(c.clock.pending()).toBeLessThanOrEqual(2);
  });

});


