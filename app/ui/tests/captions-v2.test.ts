import { get } from "svelte/store";
import { describe, expect, it, vi } from "vitest";
import {
  FADE_MS,
  MAX_HISTORY_LINES,
  MAX_OUTSTANDING_LINES,
  createCaptionController,
  isTerminal,
  visibleWords,
  type DraftDisplay,
} from "../src/lib/captions";
import fixture from "../src/lib/__fixtures__/v2/draft-display.json";
import type { CaptionLine, PipelineEvent } from "../src/lib/types";
import {
  FakeClock,
  asrFinal,
  delta,
  draft,
  dropped,
  failed,
  final,
  joined,
  partial,
  skipped,
  testConfig,
} from "./helpers";

function setup(options: { policy?: DraftDisplay; configure?: Parameters<typeof testConfig>[0] } = {}) {
  const clock = new FakeClock();
  const warn = vi.fn();
  const controller = createCaptionController({ clock, reducedMotion: () => false, onWarn: warn });
  controller.setConfig(
    testConfig((config) => {
      if (options.policy) config.overlay.draft_display = options.policy;
      options.configure?.(config);
    }),
  );
  const view = () => get(controller);
  const lines = () => view().lines;
  const line = (id: number) => lines().find((l) => l.id === id);
  const run = (...events: PipelineEvent[]) => events.forEach((event) => controller.dispatch(event));
  return { clock, warn, controller, view, lines, line, run };
}

const summary = (lines: readonly CaptionLine[]) => lines.map((l) => `${l.id}:${l.state}`);

describe("caption store (one test per row)", () => {
  it("asr_partial, no line, class chinese or mixed: adds a live line with the text", () => {
    const { lines, run } = setup();
    run(partial(1, "这次华为"), partial(2, "这个 build", "mixed"));
    expect(lines()).toMatchObject([
      { id: 1, state: "live", source: "这次华为", english: "", draft: "", drafts: [], shown: "", heldOnce: false, committed: false },
      { id: 2, state: "live", source: "这个 build", committed: false },
    ]);
  });

  it("asr_partial, no line, class english or other: adds a live line shown as heard", () => {
    const { lines, run } = setup();
    run(partial(1, "OK, can you hear", "english"), partial(2, "みんな", "other"));
    expect(lines()).toMatchObject([
      { id: 1, state: "live", source: "OK, can you hear" },
      { id: 2, state: "live", source: "みんな" },
    ]);
  });

  it("asr_partial on a live line replaces the source", () => {
    const { lines, run, clock } = setup();
    run(partial(1, "这次"));
    clock.advance(300);
    run(partial(1, "这次华为发布的呢"));
    expect(lines()).toHaveLength(1);
    expect(lines()[0]).toMatchObject({ state: "live", source: "这次华为发布的呢", updatedAt: 300 });
  });

  it("asr_partial for a line in any other state is ignored", () => {
    const { lines, run, warn } = setup();
    run(asrFinal(1, "你好。"), partial(1, "late partial"));
    run(asrFinal(2, "二"), final(2, "Two"), partial(2, "late partial"));
    run(asrFinal(3, "三"), skipped(3), partial(3, "late"));
    run(asrFinal(4, "四"), failed(4, "timeout"), partial(4, "late"));
    run(asrFinal(5, "OK", "english", "en"), partial(5, "late"));
    expect(lines().map((l) => l.source)).toEqual(["你好。", "二", "三", "四", "OK"]);
    expect(warn).toHaveBeenCalledTimes(5);
  });

  it("translation_draft on a live line stores the draft, keeps the last two and applies the policy", () => {
    const { line, run } = setup();
    run(partial(1, "这次"), draft(1, "This time", 1));
    expect(line(1)).toMatchObject({ state: "live", draft: "This time", drafts: ["This time"], shown: "This time" });
    run(draft(1, "This time, Huawei released the whole", 2));
    expect(line(1)).toMatchObject({
      draft: "This time, Huawei released the whole",
      drafts: ["This time", "This time, Huawei released the whole"],
      shown: "This time, Huawei released",
    });
    run(draft(1, "This time, Huawei released the whole family", 3));
    expect(line(1)?.drafts).toEqual(["This time, Huawei released the whole", "This time, Huawei released the whole family"]);
    expect(line(1)?.shown).toBe("This time, Huawei released the");
  });

  it("translation_draft on a pending line shows the whole draft (the clause is committed)", () => {
    const { line, run } = setup();
    run(asrFinal(1, "这次华为发布的呢。"), draft(1, "This time Huawei announced the"));
    expect(line(1)).toMatchObject({ state: "pending", committed: true, draft: "This time Huawei announced the", shown: "This time Huawei announced the" });
  });

  it("translation_draft for a terminal or unknown line is ignored", () => {
    const { lines, run, warn, line } = setup();
    run(draft(9, "nothing"));
    expect(lines()).toHaveLength(0);
    run(asrFinal(1, "一"), final(1, "One"), draft(1, "late"));
    run(asrFinal(2, "二"), skipped(2), draft(2, "late"));
    run(asrFinal(3, "三"), failed(3, "timeout"), draft(3, "late"));
    run(asrFinal(4, "four", "english", "en"), draft(4, "late"));
    run(asrFinal(5, "五"), delta(5, "Fi"), draft(5, "late"));
    expect(lines().map((l) => l.draft)).toEqual(["", "", "", "", ""]);
    expect(line(5)?.state).toBe("streaming");
    expect(warn).toHaveBeenCalledTimes(6);
  });

  it("asr_final on a live line commits it: source from the event, state by class, whole draft shown", () => {
    const { line, run } = setup();
    run(partial(1, "这次华为"), draft(1, "This time Huawei released the whole"));
    expect(line(1)?.shown).toBe("This time Huawei released");
    run(asrFinal(1, "这次华为发布的呢是整个mate90家族啊，", "chinese", "zh", "commit"));
    expect(line(1)).toMatchObject({
      state: "pending",
      committed: true,
      source: "这次华为发布的呢是整个mate90家族啊，",
      draft: "This time Huawei released the whole",
      shown: "This time Huawei released the whole",
      heldOnce: false,
      lang: "zh",
    });
    run(partial(2, "OK can you"), asrFinal(2, "OK, can you hear me?", "english", "en"));
    expect(line(2)).toMatchObject({ state: "english", committed: true, source: "OK, can you hear me?", lang: "en" });
    run(partial(3, "みんな"), asrFinal(3, "みんな、ありがとう！", "other", "ja"));
    expect(line(3)).toMatchObject({ state: "other", committed: true, lang: "ja" });
  });

  it("asr_final on a live line translates Other when the language is enabled", () => {
    const { line, run } = setup({ configure: (c) => (c.routing.translate_other = ["ja"]) });
    run(partial(1, "みんな", "other"), asrFinal(1, "みんな、ありがとう！", "other", "ja"));
    expect(line(1)?.state).toBe("pending");
  });

  it("asr_final with no line behaves as in v1 and a duplicate is ignored", () => {
    const { lines, run, warn } = setup();
    run(asrFinal(1, "你好。"));
    expect(lines()).toMatchObject([{ id: 1, state: "pending", committed: true, source: "你好。", draft: "", shown: "" }]);
    run(asrFinal(1, "你好。"));
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it("translation_delta on a line with a draft stores the text and keeps the state pending", () => {
    const { line, run } = setup();
    run(partial(1, "你好"), draft(1, "Hello there my friend"), asrFinal(1, "你好，朋友。"), delta(1, "Hello"), delta(1, "Hello there"));
    expect(line(1)).toMatchObject({ state: "pending", english: "Hello there", draft: "Hello there my friend", shown: "Hello there my friend" });
  });

  it("translation_delta on a line without a draft streams as in v1", () => {
    const { line, run } = setup();
    run(asrFinal(1, "你好。"), delta(1, "Hel"));
    expect(line(1)).toMatchObject({ state: "streaming", english: "Hel" });
  });

  it("translation_delta on a live line is ignored", () => {
    const { line, run, warn } = setup();
    run(partial(1, "你"), delta(1, "x"));
    expect(line(1)).toMatchObject({ state: "live", english: "" });
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it("translation_final sets state final and the text; the draft stays stored but is no longer the line text", () => {
    const { line, run } = setup();
    run(partial(1, "你好"), draft(1, "Hello there"), asrFinal(1, "你好。"), delta(1, "Hello"), final(1, "Hello."));
    expect(line(1)).toMatchObject({ state: "final", english: "Hello.", draft: "Hello there" });
  });

  it("skipped with a draft keeps the draft; failed with a draft keeps the draft and the v1 reason", () => {
    const { line, run } = setup();
    run(partial(1, "一"), draft(1, "One more thing"), asrFinal(1, "一。"), skipped(1));
    expect(line(1)).toMatchObject({ state: "skipped", draft: "One more thing", shown: "One more thing" });
    run(partial(2, "二"), draft(2, "Two for you"), asrFinal(2, "二。"), failed(2, "timeout"));
    expect(line(2)).toMatchObject({ state: "failed", reason: "Translation took too long", shown: "Two for you" });
  });

  it("joined removes every absorbed line in any non-terminal state; the leader keeps its draft", () => {
    const { lines, run, line } = setup();
    run(asrFinal(1, "我也想"), draft(1, "I also want to"));
    run(partial(2, "办一个"));
    run(asrFinal(3, "公司"), delta(3, "Com"));
    run(asrFinal(4, "别的"), draft(4, "Other"));
    run(asrFinal(5, "已完成"), final(5, "Done"));
    run(joined(1, [2, 3, 4, 5], "我也想，办一个，公司，别的"));
    expect(summary(lines())).toEqual(["1:pending", "5:final"]);
    expect(line(1)).toMatchObject({ source: "我也想，办一个，公司，别的", draft: "I also want to", shown: "I also want to" });
  });

  it("dropped removes a live line and does nothing for any other id", () => {
    const { lines, run, view } = setup();
    run(partial(1, "哈哈"), draft(1, "Haha"), asrFinal(2, "二"));
    run(dropped(1));
    expect(summary(lines())).toEqual(["2:pending"]);
    const before = view();
    run(dropped(2), dropped(99));
    expect(view()).toBe(before);
  });

  it("speech_started creates no line", () => {
    const { lines, run, view } = setup();
    const before = view();
    run({ type: "speech_started", id: 1, at_ms: 100 });
    expect(lines()).toHaveLength(0);
    expect(view()).toBe(before);
    run(partial(1, "你"));
    expect(summary(lines())).toEqual(["1:live"]);
  });
});

describe("expiry and limits", () => {
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

  it("a partial cancels a running fade like asr_final does", () => {
    const c = setup();
    c.run(asrFinal(1, "一"), final(1, "One"));
    c.clock.advance(8000);
    expect(c.view().fadingIds.size).toBe(1);
    c.run(partial(2, "二"));
    expect(c.view().fadingIds.size).toBe(0);
    c.clock.advance(FADE_MS * 2);
    expect(summary(c.lines())).toEqual(["1:final", "2:live"]);
  });

  it("a live line expires normally once it has settled", () => {
    const c = setup();
    c.run(partial(1, "一"), asrFinal(1, "一。"), final(1, "One."));
    c.clock.advance(8000 + FADE_MS);
    expect(c.lines()).toHaveLength(0);
  });

  it("counts live lines as outstanding and caps them", () => {
    const c = setup();
    for (let id = 1; id <= MAX_OUTSTANDING_LINES * 2; id += 1) c.run(partial(id, `句${id}`));
    expect(c.lines()).toHaveLength(MAX_OUTSTANDING_LINES);
    expect(c.lines()[0]?.id).toBe(MAX_OUTSTANDING_LINES + 1);
  });

  it("treats live as non-terminal", () => {
    expect(isTerminal("live")).toBe(false);
    expect(isTerminal("pending")).toBe(false);
    expect(isTerminal("final")).toBe(true);
    expect(isTerminal("skipped")).toBe(true);
  });
});

interface DisplayStep {
  draft?: string;
  committed?: boolean;
  visible: string;
  held: boolean;
}
interface DisplayCase {
  clip: string;
  unit: number;
  policy: DraftDisplay;
  steps: DisplayStep[];
}
const cases = fixture as unknown as DisplayCase[];

describe("draft display policy, reference/fixtures/draft-display.json", () => {
  it("contains 81 sequences: 27 draft sequences under each of the three policies", () => {
    expect(cases).toHaveLength(81);
    expect(new Set(cases.map((c) => c.policy))).toEqual(new Set(["hold2", "settled", "all"]));
    for (const policy of ["hold2", "settled", "all"]) expect(cases.filter((c) => c.policy === policy)).toHaveLength(27);
  });

  it("visibleWords reproduces every step of every sequence", () => {
    for (const c of cases) {
      const drafts: string[] = [];
      let shown: string[] | null = null;
      let heldOnce = false;
      c.steps.forEach((step, index) => {
        if (step.draft !== undefined) drafts.push(step.draft);
        const result = visibleWords(c.policy, drafts, step.committed === true, shown, heldOnce);
        const where = `${c.clip} unit ${c.unit} ${c.policy} step ${index}`;
        expect(result.words.join(" "), where).toBe(step.visible);
        expect(result.held, where).toBe(step.held);
        shown = result.words;
        heldOnce = result.held;
      });
    }
  });

  it("the store reproduces every step through events", () => {
    for (const c of cases) {
      const s = setup({ policy: c.policy });
      let rev = 0;
      s.run(partial(1, "你"));
      c.steps.forEach((step, index) => {
        if (step.committed === true) {
          s.run(asrFinal(1, "你好。", "chinese", "zh", "commit"));
        } else {
          rev += 1;
          s.run(draft(1, step.draft ?? "", rev));
        }
        const where = `${c.clip} unit ${c.unit} ${c.policy} step ${index}`;
        expect(s.line(1)?.shown, where).toBe(step.visible);
        expect(s.line(1)?.heldOnce, where).toBe(step.held);
      });
      expect(s.warn).not.toHaveBeenCalled();
      s.controller.dispose();
    }
  });
});

describe("draft display rules", () => {
  it("hold2 holds back the newest two words but always shows two", () => {
    const words = (n: number) => Array.from({ length: n }, (_, i) => `w${i + 1}`).join(" ");
    const show = (n: number) => visibleWords("hold2", [words(n)], false, null, false).words.length;
    expect([1, 2, 3, 4, 5, 8].map(show)).toEqual([1, 2, 2, 2, 3, 6]);
  });

  it("settled shows nothing for the first draft, then the common word prefix", () => {
    expect(visibleWords("settled", ["a b c"], false, null, false).words).toEqual([]);
    expect(visibleWords("settled", ["a b c", "a b d e"], false, null, false).words).toEqual(["a", "b"]);
  });

  it("the shrink hold keeps the current text once, then accepts the next update", () => {
    const s = setup({ policy: "all" });
    s.run(partial(1, "一"), draft(1, "one two three four five six", 1));
    s.run(draft(1, "uno", 2));
    expect(s.line(1)).toMatchObject({ shown: "one two three four five six", heldOnce: true, draft: "uno" });
    s.run(draft(1, "dos", 3));
    expect(s.line(1)).toMatchObject({ shown: "dos", heldOnce: false });
  });

  it("does not hold exactly half", () => {
    expect(visibleWords("all", ["a b c"], false, ["a", "b", "c", "d", "e", "f"], false)).toEqual({ words: ["a", "b", "c"], held: false });
    expect(visibleWords("all", ["a b"], false, ["a", "b", "c", "d", "e", "f"], false)).toEqual({
      words: ["a", "b", "c", "d", "e", "f"],
      held: true,
    });
  });

  it("applies a changed overlay.draft_display live to the lines already showing drafts", () => {
    const s = setup();
    s.run(partial(1, "一"), draft(1, "one two three four five", 1));
    expect(s.line(1)?.shown).toBe("one two three");
    s.controller.setConfig(testConfig((c) => (c.overlay.draft_display = "all")));
    expect(s.line(1)?.shown).toBe("one two three four five");
    s.run(draft(1, "one two three four five six seven", 2));
    expect(s.line(1)?.shown).toBe("one two three four five six seven");
    s.controller.setConfig(testConfig((c) => (c.overlay.draft_display = "settled")));
    expect(s.line(1)?.shown).toBe("one two three four five");
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
    const policies: DraftDisplay[] = ["hold2", "settled", "all"];
    const s = setup({ policy: policies[seed % 3] });
    const open = new Map<number, { rev: number }>();
    const awaiting = new Map<number, { draftAllowed: boolean }>();
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
        open.set(id, { rev: 0 });
        emit(partial(id, `句子${id}`));
      } else if (roll < 0.6 && open.size > 0) {
        const id = [...open.keys()][pick(open.size)] as number;
        const state = open.get(id) as { rev: number };
        const action = random();
        if (action < 0.35) {
          emit(partial(id, `句子${id}，更多的话`));
        } else if (action < 0.7) {
          state.rev += 1;
          emit(draft(id, text(pick(40)), state.rev));
        } else if (action < 0.9) {
          open.delete(id);
          const kind = random();
          if (kind < 0.1) emit(asrFinal(id, "OK, hello", "english", "en"));
          else if (kind < 0.2) emit(asrFinal(id, "みんな", "other", "ja"));
          else {
            emit(asrFinal(id, `句子${id}。`, "chinese", "zh", random() < 0.5 ? "commit" : "pause"));
            awaiting.set(id, { draftAllowed: true });
          }
        } else {
          open.delete(id);
          emit(dropped(id));
        }
      } else if (awaiting.size > 0) {
        const ids = [...awaiting.keys()];
        const id = ids[pick(ids.length)] as number;
        const state = awaiting.get(id) as { draftAllowed: boolean };
        const action = random();
        const line = s.line(id);
        if (action < 0.15 && state.draftAllowed && line?.state === "pending") {
          state.draftAllowed = false;
          emit(draft(id, text(pick(40)), 100));
        } else if (action < 0.4) {
          state.draftAllowed = false;
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
      expect(live).toEqual([...open.keys()].sort((a, b) => a - b));
      const active = lines.filter((l) => l.state === "pending" || l.state === "streaming").map((l) => l.id);
      expect(new Set(active)).toEqual(new Set(awaiting.keys()));
      expect(lines.length).toBeLessThanOrEqual(MAX_HISTORY_LINES + open.size + awaiting.size);
      for (const l of lines) {
        if (l.state === "live") expect(s.view().fadingIds.has(l.id)).toBe(false);
        if (l.committed && l.draft !== "") expect(l.shown).toBe(l.draft);
        if (l.draft === "") expect(l.shown).toBe("");
      }
    }
    expect(s.warn).not.toHaveBeenCalled();
    expect(events).toBeGreaterThan(2000);
    s.controller.dispose();
    expect(s.clock.pending()).toBe(0);
  });
});
