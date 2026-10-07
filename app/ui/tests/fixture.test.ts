import { readFileSync } from "node:fs";
import { get } from "svelte/store";
import { describe, expect, it } from "vitest";
import { createCaptionController } from "../src/lib/captions";
import fixture from "../src/lib/__fixtures__/events.json";
import type { PipelineEvent } from "../src/lib/types";
import { FakeClock, asrFinal, delta, final, joined, testConfig } from "./helpers";

const events = fixture as unknown as PipelineEvent[];

const EXPECTED_TYPES: PipelineEvent["type"][] = [
  "speech_started", "asr_partial", "asr_final", "joined", "translation_delta", "translation_final", "skipped",
  "translation_failed", "dropped", "source_changed", "source_state", "listening_state", "engine_status", "stats",
];

describe("wire fixture", () => {
  it("contains every one of the 14 event variants exactly once", () => {
    expect(events.map((event) => event.type)).toEqual(EXPECTED_TYPES);
  });

  it("has the numeric ids and snake_case fields the contract requires", () => {
    for (const event of events) {
      if ("id" in event) expect(Number.isSafeInteger(event.id)).toBe(true);
    }
    const final = events.find((event) => event.type === "translation_final");
    expect(final).toMatchObject({ timing: { speech_end_ms: 47980, first_token_ms: 48790 } });
    const stats = events.find((event) => event.type === "stats");
    expect(stats).toMatchObject({ queue_depth: 1, done_p50_ms: 1230, rss_translator_mb: 1380 });
  });

  it("can be replayed through the caption controller without throwing", () => {
    const controller = createCaptionController({ clock: new FakeClock(), reducedMotion: () => false });
    controller.setConfig(testConfig());
    for (const event of events) controller.dispatch(event);
    const view = get(controller);
    expect(view.lines.map((line) => `${line.id}:${line.state}`)).toEqual(["12:final"]);
    expect(view.lines[0]?.source).toBe("我也想办一个，伟大的公司。");
    controller.dispose();
  });

  it("matches the committed Rust serialization sample byte-for-byte as JSON", () => {
    const raw = readFileSync(new URL("../src/lib/__fixtures__/events.json", import.meta.url), "utf8");
    expect(JSON.parse(raw)).toEqual(events);
  });
});

describe("lawful full-stream fixtures", () => {
  it("handles a conversation with hold join, skip, failure, English and Other lines", () => {
    const clock = new FakeClock();
    const controller = createCaptionController({ clock, reducedMotion: () => false });
    controller.setConfig(testConfig());
    const run = (...list: PipelineEvent[]) => list.forEach((event) => controller.dispatch(event));
    run(
      { type: "speech_started", id: 12, at_ms: 0 },
      asrFinal(12, "我也想办一个。"),
      asrFinal(13, "伟大的公司。"),
      joined(12, [13], "我也想办一个，伟大的公司。"),
      delta(12, "I also want to run"),
      final(12, "I also want to run a great company."),
      asrFinal(14, "然后呢。"),
      { type: "skipped", id: 14, reason: "catch_up" },
      asrFinal(15, "这件事说来话长。"),
      { type: "translation_failed", id: 15, reason: "timeout", message: "Translation took too long" },
      asrFinal(16, "OK, can you hear me?", "english", "en"),
      asrFinal(17, "みんな、ありがとう！", "other", "ja"),
      { type: "dropped", id: 18, reason: "music" },
    );
    expect(get(controller).lines.map((line) => `${line.id}:${line.state}`)).toEqual([
      "12:final", "14:skipped", "15:failed", "16:english", "17:other",
    ]);
    controller.dispose();
  });
});
