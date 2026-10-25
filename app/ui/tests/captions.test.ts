import { get } from "svelte/store";
import { describe, expect, it, vi } from "vitest";
import {
  MAX_HISTORY_LINES,
  captionBlocks,
  createCaptionController,
  failReasonText,
  flowPieces,
  type CaptionBlock,
} from "../src/lib/captions";
import type { CaptionLine, FailReason, PipelineEvent } from "../src/lib/types";
import {
  FakeClock,
  asrFinal,
  delta,
  dropped,
  failed,
  final,
  sealed,
  partial,
  skipped,
  testConfig,
} from "./helpers";

function setup(options: { configure?: Parameters<typeof testConfig>[0] } = {}) {
  const clock = new FakeClock();
  const warn = vi.fn();
  const controller = createCaptionController({ clock, onWarn: warn });
  controller.setConfig(testConfig(options.configure));
  const view = () => get(controller);
  const blocks = () => view().blocks;
  const run = (...events: PipelineEvent[]) => events.forEach((event) => controller.dispatch(event));
  return { clock, warn, controller, view, blocks, run };
}

const summary = (blocks: readonly CaptionBlock[]) => blocks.map((block) => `${block.key}:${block.state}`);

describe("caption blocks", () => {
  const line = (id: number, state: CaptionLine["state"], source: string, english = ""): CaptionLine => ({
    id,
    state,
    source,
    english,
    reason: null,
    updatedAt: 0,
  });
  const seals = (...pairs: [number, number[]][]) => new Map(pairs.map(([id, ids]) => [id, { id, ids }]));

  it("keeps the first line as the key when the core seals a unit again", () => {
    const lines = [line(10, "pending", "我也想，办一个，公司"), line(13, "live", "别的话")];
    const one = captionBlocks(lines, seals([10, [10, 11]]), null, true);
    expect(one.map((block) => [block.key, block.ids])).toEqual([[10, [10, 11]], [13, [13]]]);
    const two = captionBlocks(lines, seals([10, [10, 11, 12]]), null, true);
    expect(two.map((block) => [block.key, block.ids])).toEqual([[10, [10, 11, 12]], [13, [13]]]);
  });

  it("never splits a block for a preview, whichever lines it covers", () => {
    const lines = [line(2, "pending", "今天天气很好，"), line(3, "live", "我们去")];
    const preview = { id: 2, ids: [2, 3], text: "The weather is nice today, we go" };
    const before = captionBlocks(lines, new Map(), preview, true);
    expect(before).toHaveLength(1);
    expect(before[0]).toMatchObject({ key: 2, ids: [2, 3], preview: "The weather is nice today, we go" });
    const shrunk = captionBlocks(lines, new Map(), { ...preview, text: "The weather" }, true);
    expect(shrunk.map((block) => block.key)).toEqual([2]);
  });

  it("opens a new block for the lines after a seal", () => {
    const lines = [line(10, "pending", "第一句。"), line(11, "pending", "第二句。")];
    const blocks = captionBlocks(lines, seals([10, [10]]), null, true);
    expect(blocks.map((block) => [block.key, block.ids, block.state])).toEqual([
      [10, [10], "pending"],
      [11, [11], "pending"],
    ]);
  });

  it("leaves no block for a dropped live line", () => {
    const { blocks, run } = setup();
    run(partial(1, "哈哈"), dropped(1));
    expect(blocks()).toEqual([]);
    run(partial(2, "我们"));
    expect(blocks().map((block) => block.key)).toEqual([2]);
  });
});

describe("caption reducer", () => {
  it("walks pending -> streaming -> final and keeps the source", () => {
    const { blocks, run } = setup();
    run(asrFinal(1, "你好。"));
    expect(blocks()).toMatchObject([{ key: 1, state: "pending", source: "你好。", english: "", reason: null }]);
    run(delta(1, "Hel"));
    expect(blocks()).toMatchObject([{ state: "streaming", english: "Hel", source: "你好。" }]);
    run(final(1, "Hello."));
    expect(blocks()).toMatchObject([{ state: "final", english: "Hello." }]);
  });

  it("seals absorbed ids into the leader, which keeps the earliest position", () => {
    const { blocks, run } = setup();
    run(asrFinal(10, "我也想"), asrFinal(11, "办一个"), asrFinal(12, "公司"), asrFinal(13, "别的话"));
    expect(blocks()).toMatchObject([{ key: 10, ids: [10, 11, 12, 13], state: "pending" }]);
    run(sealed(10, [10, 11], "我也想，办一个"));
    expect(summary(blocks())).toEqual(["10:pending", "12:pending"]);
    expect(blocks()[0]?.source).toBe("我也想，办一个");
    run(sealed(10, [10, 11, 12], "我也想，办一个，公司"));
    expect(summary(blocks())).toEqual(["10:pending", "13:pending"]);
    expect(blocks()[0]).toMatchObject({ key: 10, ids: [10, 11, 12], source: "我也想，办一个，公司" });
    run(delta(10, "I also"), final(10, "I also want to run a company."));
    expect(blocks()[0]).toMatchObject({ state: "final", source: "我也想，办一个，公司" });
  });

  // The English comes from the corrected text; a row that kept the misheard word would contradict it.
  it("shows a one-character correction from the second pass", () => {
    const { blocks, run } = setup();
    run(partial(40, "我也想半一个"));
    run({
      ...asrFinal(40, "我也想办一个"),
      second_pass: {
        outcome: "used",
        live_text: "我也想半一个",
        window_start_ms: 0,
        window_end_ms: 1800,
        distance: 1,
        decode_ms: 500,
        refine_ms: 600,
      },
    } as PipelineEvent);
    expect(blocks()).toMatchObject([{ key: 40, state: "pending", source: "我也想办一个" }]);
    expect(blocks()).toHaveLength(1);
  });

  it("ignores duplicate asr_final and anything after a terminal event", () => {
    const { blocks, run } = setup();
    run(asrFinal(31, "你好"), final(31, "Hello"), asrFinal(31, "你好"), delta(31, "Hel"), failed(31, "timeout"), skipped(31));
    expect(blocks()).toMatchObject([{ state: "final", english: "Hello" }]);
  });

  it.each<[FailReason, string, string]>([
    ["timeout", "ignored", "Translation took too long"],
    ["server_unavailable", "ignored", "Translator is restarting"],
    ["echo", "ignored", "Could not translate this line"],
    ["runaway", "ignored", "Could not translate this line"],
    ["error", "model exploded", "model exploded"],
    ["error", "   ", "Could not translate this line"],
  ])("maps failure %s to a user-facing reason", (reason, message, expected) => {
    const { blocks, run } = setup();
    run(asrFinal(60, "这件事说来话长。"), delta(60, "It is"), failed(60, reason, message));
    expect(blocks()[0]).toMatchObject({ state: "failed", reason: expected, source: "这件事说来话长。" });
    expect(failReasonText(reason, message)).toBe(expected);
  });

  it("dropped removes a live line and does nothing for any other id", () => {
    const { blocks, run, view, warn } = setup();
    run(partial(1, "哈哈"), asrFinal(2, "二"));
    run(dropped(1));
    expect(summary(blocks())).toEqual(["2:pending"]);
    const before = view();
    run(dropped(2), dropped(99));
    expect(view()).toBe(before);
    expect(warn).not.toHaveBeenCalled();
  });

});

describe("running text", () => {
  // A block that drops out or swaps places here would show wrong English with no error anywhere.
  it("keeps each block in order, with live Chinese and pending English at the end", () => {
    const { blocks, run } = setup();
    run(asrFinal(1, "你好。"), sealed(1, [1], "你好。"), final(1, "Hello."));
    run(asrFinal(2, "Okay.", "english"));
    run(asrFinal(3, "然后吃饭。"), sealed(3, [3], "然后吃饭。"), delta(3, "Then"));
    run(asrFinal(4, "好的。"), sealed(4, [4], "好的。"));
    run(partial(5, "我们"));
    const flow = flowPieces(blocks(), true);
    expect(flow.source.map((piece) => [piece.text, piece.kind])).toEqual([
      ["你好。", "final"],
      ["然后吃饭。", "final"],
      ["好的。", "final"],
      ["我们", "live"],
    ]);
    expect(flow.english.map((piece) => [piece.text, piece.kind])).toEqual([
      ["Hello.", "final"],
      ["Okay.", "english"],
      ["Then", "streaming"],
      ["···", "pending"],
    ]);
    const hidden = flowPieces(blocks(), false);
    expect(hidden.source.map((piece) => piece.kind)).toEqual(["live"]);
    expect(hidden.english).toEqual(flow.english);
  });

  // The preview stands for every block it covers; shown next to their own English or dots, the
  // same words would appear twice, and a lost preview would leave the reader with nothing.
  it("shows one preview for the blocks it covers until their final translation", () => {
    const { blocks, run } = setup({ configure: (config) => { config.overlay.style = "flow"; } });
    const english = () => flowPieces(blocks(), true).english.map((piece) => [piece.text, piece.kind]);
    run(asrFinal(1, "你好。"), final(1, "Hello."));
    run(asrFinal(2, "今天天气很好，"), partial(3, "我们去"));
    run({ type: "translation_preview", id: 2, ids: [2, 3], text: "The weather is nice today, we go" });
    expect(english()).toEqual([
      ["Hello.", "final"],
      ["The weather is nice today, we go", "preview"],
    ]);
    run(asrFinal(3, "我们去公园。"), sealed(2, [2, 3], "今天天气很好，我们去公园。"), delta(2, "The weather"));
    expect(english()).toEqual([
      ["Hello.", "final"],
      ["The weather is nice today, we go", "preview"],
    ]);
    run(final(2, "The weather is nice today, let's go to the park."));
    expect(english()).toEqual([
      ["Hello.", "final"],
      ["The weather is nice today, let's go to the park.", "final"],
    ]);
  });

  // The panel shows the covered lines as the one block they become; left as separate rows, their
  // English would be missing or shown twice.
  it("shows the lines a preview covers as one panel block until their final translation", () => {
    const { blocks, run } = setup();
    const rows = () => blocks().map((block) => [block.key, block.source, block.preview ?? block.english]);
    run(asrFinal(1, "你好。"), sealed(1, [1], "你好。"), final(1, "Hello."));
    run(asrFinal(2, "今天天气很好，"), partial(3, "我们去"));
    run({ type: "translation_preview", id: 2, ids: [2, 3], text: "The weather is nice today, we go" });
    expect(rows()).toEqual([
      [1, "你好。", "Hello."],
      [2, "今天天气很好，", "The weather is nice today, we go"],
    ]);
    const { blocks: hidden, run: runHidden } = setup({ configure: (config) => { config.overlay.live_source = false; } });
    runHidden(asrFinal(1, "你好。"), sealed(1, [1], "你好。"), final(1, "Hello."));
    runHidden(asrFinal(2, "今天天气很好，"), partial(3, "我们去"));
    expect(hidden().map((block) => [block.source, block.live])).toEqual([["你好。", ""], ["今天天气很好，", ""]]);
    run(asrFinal(3, "我们去公园。"), sealed(2, [2, 3], "今天天气很好，我们去公园。"));
    run(final(2, "The weather is nice today, let's go to the park."));
    expect(rows()).toEqual([
      [1, "你好。", "Hello."],
      [2, "今天天气很好，我们去公园。", "The weather is nice today, let's go to the park."],
    ]);
  });

  it.each(["flow", "panel"] as const)("keeps its text through silence (%s)", (style) => {
    const c = setup({ configure: (config) => { config.overlay.style = style; } });
    c.run(asrFinal(1, "一"), final(1, "One"));
    c.clock.advance(600_000);
    expect(summary(c.blocks())).toEqual(["1:final"]);
  });
});

describe("history", () => {
  it("bounds history and outstanding rows during an hour of continuous speech", () => {
    const c = setup();
    let peak = 0;
    for (let id = 1; id <= 900; id += 1) {
      c.run(asrFinal(id, `句子${id}`));
      c.clock.advance(1500);
      c.run(delta(id, "So"), final(id, "Some text."));
      c.clock.advance(2500);
      peak = Math.max(peak, c.blocks().length);
      expect(c.blocks().length).toBeLessThanOrEqual(MAX_HISTORY_LINES + 1);
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
          if (kind < 0.1) emit(asrFinal(id, "OK, hello", "english"));
          else {
            emit(asrFinal(id, `句子${id}。`, "chinese", random() < 0.5 ? "commit" : "pause"));
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
            emit(sealed(id, [id, other], `合并${id}`));
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

      const blocks = s.blocks();
      const holders = new Map<number, CaptionBlock[]>();
      for (const block of blocks) {
        for (const id of block.ids) holders.set(id, [...(holders.get(id) ?? []), block]);
      }
      for (const [, held] of holders) expect(held).toHaveLength(1);
      const unfinished = new Set([...open, ...awaiting]);
      for (const id of unfinished) {
        const [block] = holders.get(id) ?? [];
        expect(block !== undefined && !["final", "failed", "skipped", "english"].includes(block.state)).toBe(true);
      }
      for (const id of open) expect(holders.get(id)?.[0]?.sealed).toBe(false);
      expect(blocks.length).toBeLessThanOrEqual(MAX_HISTORY_LINES + open.size + awaiting.size);
    }
    expect(s.warn).not.toHaveBeenCalled();
    expect(events).toBeGreaterThan(2000);
    s.controller.dispose();
    expect(s.clock.pending()).toBe(0);
  });
});
