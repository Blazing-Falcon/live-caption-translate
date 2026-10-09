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

  it("replaces repeated full-text deltas instead of concatenating them", () => {
    const { lines, run } = setup();
    run(asrFinal(1, "我们走吧。"), delta(1, "Let's"), delta(1, "Let's go"), delta(1, "Let's go"), delta(1, "Let's go now"));
    expect(lines()[0]?.english).toBe("Let's go now");
    expect(lines()[0]?.state).toBe("streaming");
  });

  it("shows English speech as heard, terminal, with no translation", () => {
    const { lines, run } = setup();
    run(asrFinal(2, "OK, can you hear me?", "english", "en"), delta(2, "ignored"), final(2, "ignored"));
    expect(lines()).toMatchObject([{ id: 2, state: "english", source: "OK, can you hear me?", english: "", lang: "en" }]);
  });

  it("treats Mixed like Chinese", () => {
    const { lines, run } = setup();
    run(asrFinal(3, "这个 build 很稳。", "mixed", "zh"));
    expect(lines()[0]?.state).toBe("pending");
  });

  it("keeps Other-language text untranslated when the language is not enabled", () => {
    const { lines, run } = setup();
    run(asrFinal(4, "みんな、ありがとう！", "other", "ja"));
    expect(lines()).toMatchObject([{ state: "other", lang: "ja", source: "みんな、ありがとう！" }]);
  });

  it("translates Other language when it is enabled, normalizing raw tags", () => {
    const { lines, run, controller } = setup({ configure: (c) => (c.routing.translate_other = ["ja"]) });
    run(asrFinal(5, "ありがとう", "other", "<|ja|>"), asrFinal(6, "감사합니다", "other", "ko"), asrFinal(7, "ありがとう", "other", "ja-JP"));
    expect(summary(lines())).toEqual(["5:pending", "6:other", "7:pending"]);
    expect(lines()[0]?.lang).toBe("ja");
    controller.setConfig(testConfig((c) => (c.routing.translate_other = ["ja", "ko"])));
    run(asrFinal(8, "감사합니다", "other", "ko"));
    expect(lines()[3]?.state).toBe("pending");
  });

  it("treats Other with an unknown language as not enabled", () => {
    const { lines, run } = setup({ configure: (c) => (c.routing.translate_other = ["ja"]) });
    run(asrFinal(9, "???", "other", null));
    expect(lines()[0]).toMatchObject({ state: "other", lang: null });
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

  it("never recreates absorbed rows and ignores joins for unknown or settled leaders", () => {
    const { lines, run, warn } = setup();
    run(joined(20, [21], "text"));
    expect(lines()).toHaveLength(0);
    run(asrFinal(22, "一"), joined(22, [99, 22], "一二"));
    expect(summary(lines())).toEqual(["22:pending"]);
    expect(lines()[0]?.source).toBe("一二");
    run(final(22, "One two."), joined(22, [], "late"));
    expect(lines()[0]).toMatchObject({ state: "final", source: "一二" });
    expect(warn).toHaveBeenCalled();
  });

  it("ignores events for unknown ids", () => {
    const { lines, run, warn } = setup();
    run(delta(30, "x"), final(30, "x"), failed(30, "timeout"), skipped(30));
    expect(lines()).toHaveLength(0);
    expect(warn).toHaveBeenCalledTimes(4);
  });

  it("ignores duplicate asr_final and anything after a terminal event", () => {
    const { lines, run } = setup();
    run(asrFinal(31, "你好"), final(31, "Hello"), asrFinal(31, "你好"), delta(31, "Hel"), failed(31, "timeout"), skipped(31));
    expect(lines()).toMatchObject([{ state: "final", english: "Hello" }]);
  });

  it("does not create or change rows for speech_started, asr_partial, dropped or stats", () => {
    const { lines, run, view } = setup();
    run(asrFinal(40, "你好"));
    const before = view();
    run(
      { type: "speech_started", id: 41, at_ms: 5 },
      { type: "asr_partial", id: 40, text: "你", class: "chinese", end_ms: 100 },
      { type: "dropped", id: 42, reason: "empty" },
      { type: "stats", ...STATS },
    );
    expect(view()).toBe(before);
    expect(lines()).toHaveLength(1);
  });

  it("marks skipped lines and keeps the source", () => {
    const { lines, run } = setup();
    run(asrFinal(50, "然后我们再去下一个地图看看。"), skipped(50));
    expect(lines()[0]).toMatchObject({ state: "skipped", source: "然后我们再去下一个地图看看。", english: "" });
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

  it("keeps insertion order when later events touch an earlier id", () => {
    const { lines, run } = setup();
    run(asrFinal(1, "一"), asrFinal(2, "二"), asrFinal(3, "三"), delta(1, "One"), final(2, "Two"), delta(1, "One more"));
    expect(summary(lines())).toEqual(["1:streaming", "2:final", "3:pending"]);
  });

  it("applies the age/opacity rule and the visible window per style", () => {
    const sample = Array.from({ length: 8 }, (_, id) => ({ id }) as CaptionLine);
    expect(visibleLines(sample, "bar", 5).map((l) => l.id)).toEqual([6, 7]);
    expect(visibleLines(sample, "panel", 5).map((l) => l.id)).toEqual([3, 4, 5, 6, 7]);
    expect(visibleLines(sample, "panel", 99)).toHaveLength(6);
    expect(visibleLines(sample, "panel", 1)).toHaveLength(4);
    expect([0, 1, 2].map((age) => ageOpacity("bar", age))).toEqual([1, 0.55, 0.55]);
    expect([0, 1, 2, 3, 9].map((age) => ageOpacity("panel", age))).toEqual([1, 0.75, 0.6, 0.45, 0.45]);
  });
});

describe("expiry", () => {
  const settle = (c: ReturnType<typeof setup>, id: number) => {
    c.run(asrFinal(id, `行${id}`), final(id, `Line ${id}`));
  };

  it("fades terminal lines after expire_s without asr_final, then removes them after 200 ms", () => {
    const c = setup();
    settle(c, 1);
    c.clock.advance(7999);
    expect(c.view().fadingIds.size).toBe(0);
    c.clock.advance(1);
    expect([...c.view().fadingIds]).toEqual([1]);
    expect(c.lines()).toHaveLength(1);
    c.clock.advance(FADE_MS - 1);
    expect(c.lines()).toHaveLength(1);
    c.clock.advance(1);
    expect(c.lines()).toHaveLength(0);
    expect(c.view().fadingIds.size).toBe(0);
    expect(c.clock.pending()).toBe(0);
  });

  it("honours a changed expire_s and does not restart the clock on deltas or stats", () => {
    const c = setup({ configure: (cfg) => (cfg.overlay.expire_s = 3) });
    c.run(asrFinal(1, "一"), asrFinal(2, "二"), final(1, "One"));
    c.clock.advance(2000);
    c.run(delta(2, "Tw"), { type: "stats", ...STATS });
    c.clock.advance(1000);
    expect([...c.view().fadingIds]).toEqual([1]);
    c.clock.advance(FADE_MS);
    expect(summary(c.lines())).toEqual(["2:streaming"]);
  });

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

  it("cancels a running fade when a new asr_final arrives and restarts the quiet period", () => {
    const c = setup();
    settle(c, 1);
    c.clock.advance(8000);
    expect(c.view().fadingIds.size).toBe(1);
    c.clock.advance(100);
    c.run(asrFinal(2, "二"));
    expect(c.view().fadingIds.size).toBe(0);
    c.clock.advance(FADE_MS * 2);
    expect(summary(c.lines())).toEqual(["1:final", "2:pending"]);
    c.run(final(2, "Two"));
    c.clock.advance(16100 - c.clock.time - 1);
    expect(c.view().fadingIds.size).toBe(0);
    c.clock.advance(1);
    expect([...c.view().fadingIds].sort()).toEqual([1, 2]);
  });

  it("removes immediately under reduced motion", () => {
    const c = setup({ reducedMotion: true });
    settle(c, 1);
    c.clock.advance(8000);
    expect(c.lines()).toHaveLength(0);
    expect(c.view().fadingIds.size).toBe(0);
    expect(c.clock.pending()).toBe(0);
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

  it("caps rows that never receive a terminal event", () => {
    const c = setup();
    for (let id = 1; id <= MAX_OUTSTANDING_LINES * 3; id += 1) c.run(asrFinal(id, `句${id}`));
    expect(c.lines()).toHaveLength(MAX_OUTSTANDING_LINES);
    expect(c.lines()[0]?.id).toBe(MAX_OUTSTANDING_LINES * 2 + 1);
  });

  it("drops every timer and ignores events after dispose", () => {
    const c = setup();
    c.run(asrFinal(1, "一"), final(1, "One"), { type: "listening_state", state: "listening" });
    expect(c.clock.pending()).toBeGreaterThan(0);
    const seen: number[] = [];
    const stop = c.controller.subscribe((v) => seen.push(v.lines.length));
    c.controller.dispose();
    expect(c.clock.pending()).toBe(0);
    const count = seen.length;
    c.run(asrFinal(2, "二"));
    c.controller.setConfig(testConfig());
    c.clock.advance(60_000);
    expect(seen).toHaveLength(count);
    expect(c.clock.pending()).toBe(0);
    stop();
  });

  it("works with the default system clock under vitest fake timers", () => {
    vi.useFakeTimers();
    try {
      const controller: CaptionController = createCaptionController({ reducedMotion: () => false });
      controller.setConfig(testConfig((cfg) => (cfg.overlay.expire_s = 3)));
      controller.dispatch(asrFinal(1, "一"));
      controller.dispatch(final(1, "One"));
      vi.advanceTimersByTime(3000);
      expect(get(controller).fadingIds.size).toBe(1);
      vi.advanceTimersByTime(FADE_MS);
      expect(get(controller).lines).toHaveLength(0);
      controller.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("status chip", () => {
  it("shows Listening for 3 seconds on the transition, using the source label", () => {
    const c = setup();
    c.run({ type: "source_changed", info: { mode: "system", label: "Speakers (Realtek)", sample_rate: 48000, channels: 2 } });
    c.run({ type: "listening_state", state: "listening" });
    expect(c.view().chip?.text).toBe("Listening · Speakers (Realtek)");
    c.clock.advance(CHIP_MS - 1);
    expect(c.view().chip).not.toBeNull();
    c.clock.advance(1);
    expect(c.view().chip).toBeNull();
    expect(c.clock.pending()).toBe(0);
  });

  it("shows 'No audio playing on {label}' once when silent starts and no lines exist", () => {
    const c = setup();
    c.run({ type: "source_changed", info: { mode: "apps", label: "Chrome, Discord", sample_rate: 48000, channels: 2 } });
    c.run({ type: "source_state", state: "silent", detail: null });
    expect(c.view().chip?.text).toBe("No audio playing on Chrome, Discord");
    expect(c.lines()).toHaveLength(0);
    c.clock.advance(CHIP_MS);
    c.run({ type: "source_state", state: "silent", detail: null }, { type: "stats", ...STATS });
    expect(c.view().chip).toBeNull();
  });

  it("omits the dangling 'on' without a source label and skips the chip when lines are visible", () => {
    const c = setup();
    c.run({ type: "source_state", state: "silent", detail: null });
    expect(c.view().chip?.text).toBe("No audio playing");
    const d = setup();
    d.run(asrFinal(1, "你好"), { type: "source_state", state: "silent", detail: null });
    expect(d.view().chip).toBeNull();
  });

  it("replaces an earlier chip and restarts the timer", () => {
    const c = setup();
    c.run({ type: "listening_state", state: "listening" });
    c.clock.advance(1000);
    c.run({ type: "source_state", state: "silent", detail: null });
    const second = c.view().chip;
    expect(second?.text).toBe("No audio playing");
    c.clock.advance(2500);
    expect(c.view().chip).toBe(second);
    c.clock.advance(500);
    expect(c.view().chip).toBeNull();
    expect(c.clock.pending()).toBe(0);
  });

  it("does not show a chip for Paused or for duplicate listening states", () => {
    const c = setup();
    c.run({ type: "listening_state", state: "listening" });
    c.clock.advance(CHIP_MS);
    c.run({ type: "listening_state", state: "listening" });
    expect(c.view().chip).toBeNull();
    c.run({ type: "listening_state", state: "paused" });
    expect(c.view().chip).toBeNull();
  });

  it("hides the chip while a status line replaces the captions", () => {
    const c = setup();
    c.run({ type: "listening_state", state: "listening" });
    c.run({ type: "engine_status", engine: "asr", state: "failed", message: null });
    expect(c.view().status?.kind).toBe("engine_failed");
    expect(c.view().chip).toBeNull();
    c.run({ type: "engine_status", engine: "asr", state: "ready", message: null });
    expect(c.view().chip?.text).toBe("Listening");
  });
});

describe("status integration", () => {
  it("derives status from app state and events and announces it politely", () => {
    const c = setup();
    c.controller.setAppState(testState({ models_ready: false }));
    expect(c.view().status?.text).toBe("Models not downloaded. Open settings to download.");
    expect(c.view().announcement).toBe("Models not downloaded. Open settings to download.");
    c.controller.setAppState(testState());
    expect(c.view().status).toBeNull();
    c.run({ type: "engine_status", engine: "translator", state: "restarting", message: null });
    expect(c.view().status?.text).toBe("Restarting translator…");
    c.run({ type: "listening_state", state: "paused" });
    expect(c.view().status?.text).toBe("Restarting translator…");
    c.run({ type: "engine_status", engine: "translator", state: "ready", message: null });
    expect(c.view().status?.text).toBe("Paused · Ctrl Shift P to resume");
  });

  it("uses the configured pause binding and never prints an empty shortcut", () => {
    const c = setup({ configure: (cfg) => (cfg.hotkeys.pause = "Alt+F9") });
    c.controller.setAppState(testState({ listening: "paused" }));
    expect(c.view().status?.text).toBe("Paused · Alt F9 to resume");
    c.controller.setConfig(testConfig((cfg) => (cfg.hotkeys.pause = "")));
    expect(c.view().status?.text).toBe("Paused · Resume in Settings");
  });

  it("keeps lines underneath a status line and shows them again afterwards", () => {
    const c = setup();
    c.controller.setAppState(testState());
    c.run(asrFinal(1, "你好"), final(1, "Hello"));
    c.run({ type: "source_state", state: "no_device", detail: null });
    expect(c.view().status?.text).toBe("No audio device");
    expect(c.lines()).toHaveLength(1);
    c.run({ type: "source_state", state: "playing", detail: null });
    expect(c.view().status).toBeNull();
  });

  it("announces final captions once per terminal line", () => {
    const c = setup();
    c.run(asrFinal(1, "你好"), delta(1, "Hel"));
    expect(c.view().announcement).toBe("");
    c.run(final(1, "Hello."));
    expect(c.view().announcement).toBe("Hello.");
  });
});
