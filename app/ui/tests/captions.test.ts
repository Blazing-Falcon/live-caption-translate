import { get } from "svelte/store";
import { describe, expect, it, vi } from "vitest";
import { FADE_MS, MAX_HISTORY_LINES, createCaptionController, failReasonText, flowText } from "../src/lib/captions";
import type { CaptionLine, FailReason, PipelineEvent } from "../src/lib/types";
import {
  FakeClock,
  asrFinal,
  delta,
  dropped,
  failed,
  final,
  joined,
  partial,
  skipped,
  testConfig,
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
    run(joined(10, [11], "我也想，办一个"));
    expect(summary(lines())).toEqual(["10:pending", "12:pending", "13:pending"]);
    expect(lines()[0]?.source).toBe("我也想，办一个");
    run(joined(10, [12], "我也想，办一个，公司"));
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

  it("dropped removes a live line and does nothing for any other id", () => {
    const { lines, run, view, warn } = setup();
    run(partial(1, "哈哈"), asrFinal(2, "二"));
    run(dropped(1));
    expect(summary(lines())).toEqual(["2:pending"]);
    const before = view();
    run(dropped(2), dropped(99));
    expect(view()).toBe(before);
    expect(warn).not.toHaveBeenCalled();
  });

});

describe("running text", () => {
  // A line that drops out or swaps places here would show wrong English with no error anywhere.
  it("keeps each line in order, with live Chinese and pending English at the end", () => {
    const { lines, run } = setup();
    run(asrFinal(1, "你好。"), final(1, "Hello."));
    run(asrFinal(2, "Okay.", "english", "en"));
    run(asrFinal(3, "然后吃饭。"), delta(3, "Then"));
    run(asrFinal(4, "好的。"));
    run(partial(5, "我们"));
    const flow = flowText(lines(), true, true);
    expect(flow.source.map((piece) => [piece.text, piece.kind])).toEqual([
      ["你好。", "final"],
      ["然后吃饭。", "final"],
      ["好的。", "final"],
      ["我们", "live"],
    ]);
    expect(flow.english.map((piece) => [piece.text, piece.kind])).toEqual([
      ["Hello.", "final"],
      ["Okay.", "final"],
      ["Then", "streaming"],
      ["···", "pending"],
    ]);
    const hidden = flowText(lines(), false, false);
    expect(hidden.source).toEqual([]);
    expect(hidden.english).toEqual(flow.english);
  });
});

describe("expiry", () => {
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

  it("never expires a live line, and a partial resets the expiry clock", () => {
    const c = setup();
    c.run(asrFinal(1, "一"), final(1, "One"), partial(2, "二"));
    c.clock.advance(5000);
    c.run(partial(2, "二二"));
    c.clock.advance(7999);
    expect(c.view().fadingIds.size).toBe(0);
    c.clock.advance(1);
    expect([...c.view().fadingIds]).toEqual([1]);
    c.clock.advance(FADE_MS);
    expect(summary(c.lines())).toEqual(["2:live"]);
    c.clock.advance(600_000);
    expect(summary(c.lines())).toEqual(["2:live"]);
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



/** Small deterministic generator (mulberry32). */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

describe("long synthetic event log", () => {
  const ENGLISH = "This time the company released the whole family of new phones and a few more things".split(" ");

  it.each([1, 2, 3])("drives the store lawfully for thousands of events without warnings (seed %i)", (seed) => {
    const random = rng(seed);
    const pick = (n: number) => Math.floor(random() * n);
    const s = setup();
    const open = new Set<number>();
    const awaiting = new Set<number>();
    let nextId = 1;
    let events = 0;
    const emit = (event: PipelineEvent) => {
      s.controller.dispatch(event);
      events += 1;
    };
    const text = (n: number) => ENGLISH.slice(0, 1 + (n % ENGLISH.length)).join(" ");

    for (let step = 0; step < 6000; step += 1) {
      const roll = random();
      if (roll < 0.2 && open.size < 3) {
        const id = nextId++;
        open.add(id);
        emit(partial(id, `句子${id}`));
      } else if (roll < 0.6 && open.size > 0) {
        const id = [...open][pick(open.size)] as number;
        const action = random();
        if (action < 0.6) {
          emit(partial(id, `句子${id}，更多的话`));
        } else if (action < 0.9) {
          open.delete(id);
          const kind = random();
          if (kind < 0.1) emit(asrFinal(id, "OK, hello", "english", "en"));
          else if (kind < 0.2) emit(asrFinal(id, "みんな", "other", "ja"));
          else {
            emit(asrFinal(id, `句子${id}。`, "chinese", "zh", random() < 0.5 ? "commit" : "pause"));
            awaiting.add(id);
          }
        } else {
          open.delete(id);
          emit(dropped(id));
        }
      } else if (awaiting.size > 0) {
        const ids = [...awaiting];
        const id = ids[pick(ids.length)] as number;
        const action = random();
        if (action < 0.4) {
          emit(delta(id, text(pick(10))));
        } else if (action < 0.55 && ids.length > 1) {
          const other = ids.find((candidate) => candidate > id);
          if (other !== undefined) {
            awaiting.delete(other);
            emit(joined(id, [other], `合并${id}`));
          }
        } else if (action < 0.85) {
          awaiting.delete(id);
          emit(final(id, text(pick(40))));
        } else if (action < 0.93) {
          awaiting.delete(id);
          emit(skipped(id));
        } else {
          awaiting.delete(id);
          emit(failed(id, "timeout"));
        }
      }
      s.clock.advance(pick(1500));

      const lines = s.lines();
      const live = lines.filter((l) => l.state === "live").map((l) => l.id).sort((a, b) => a - b);
      expect(live).toEqual([...open].sort((a, b) => a - b));
      const active = lines.filter((l) => l.state === "pending" || l.state === "streaming").map((l) => l.id);
      expect(new Set(active)).toEqual(awaiting);
      expect(lines.length).toBeLessThanOrEqual(MAX_HISTORY_LINES + open.size + awaiting.size);
      for (const l of lines) {
        if (l.state === "live") expect(s.view().fadingIds.has(l.id)).toBe(false);
      }
    }
    expect(s.warn).not.toHaveBeenCalled();
    expect(events).toBeGreaterThan(2000);
    s.controller.dispose();
    expect(s.clock.pending()).toBe(0);
  });
});
