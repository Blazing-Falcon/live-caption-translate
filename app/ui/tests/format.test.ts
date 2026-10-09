import { describe, expect, it } from "vitest";
import {
  footerText,
  formatBytes,
  formatMemory,
  formatSeconds,
  levelText,
  modeStatusText,
  modelTitle,
  modelsAllReady,
  modelsPercent,
  totalCpu,
  totalMemoryMb,
} from "../src/lib/format";
import { STATS } from "./helpers";

describe("format", () => {
  it("formats byte sizes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(2048)).toBe("2 KB");
    expect(formatBytes(2 * 1024 * 1024)).toBe("2.0 MB");
    expect(formatBytes(239 * 1024 * 1024)).toBe("239 MB");
    expect(formatBytes(1.08 * 1024 ** 3)).toBe("1.08 GB");
  });

  it("formats memory and seconds", () => {
    expect(formatMemory(520)).toBe("520 MB");
    expect(formatMemory(2660)).toBe("2.6 GB");
    expect(formatSeconds(1800)).toBe("1.8 s");
    expect(formatSeconds(null)).toBe("—");
    expect(formatSeconds(undefined)).toBe("—");
  });

  it("converts peak to a readable level", () => {
    expect(levelText({ peak: 0, active: true })).toBe("silent");
    expect(levelText({ peak: 0.5, active: true })).toBe("-6 dB");
    expect(levelText({ peak: 1, active: true })).toBe("0 dB");
    expect(levelText({ peak: 0.01, active: true })).toBe("-40 dB");
    expect(levelText({ peak: 0.5, active: false })).toBe("not playing");
  });

  it("sums CPU and memory across app, translator and the draft server", () => {
    expect(totalCpu(STATS)).toBe(41);
    expect(totalCpu({ cpu_app_pct: 80, cpu_translator_pct: 150, cpu_draft_pct: 0 })).toBe(230);
    expect(totalCpu({ cpu_app_pct: 2.5, cpu_translator_pct: 38.4, cpu_draft_pct: 55 })).toBe(96);
    expect(totalMemoryMb(STATS)).toBe(1900);
    expect(totalMemoryMb({ ...STATS, rss_draft_mb: 610 })).toBe(2510);
  });

  it("words the Performance status line for every mode and reason", () => {
    expect(modeStatusText("continuous", null)).toBe("Now: Continuous");
    expect(modeStatusText("continuous", "auto")).toBe("Now: Continuous");
    expect(modeStatusText("continuous", "user")).toBe("Now: Continuous");
    expect(modeStatusText("light", null)).toBe("Now: Light");
    expect(modeStatusText("light", "auto")).toBe("Now: Light");
    expect(modeStatusText("light", "user")).toBe("Now: Light");
    expect(modeStatusText("light", "cpu")).toBe("Now: Light, because the PC is busy");
    expect(modeStatusText("light", "lag")).toBe("Now: Light, because translation fell behind");
    expect(modeStatusText("light", "draft_unavailable")).toBe("Now: Light, because the draft model is not downloaded");
    expect(modeStatusText("off", null)).toBe("Now: Off");
    expect(modeStatusText("off", "user")).toBe("Now: Off");
  });

  it("names the draft model for the first-run list and ignores optional models when deciding readiness", () => {
    expect(modelTitle({ id: "lmt-60-0.6b-q4_k_m", name: "Faster captions (LMT-60 0.6B, Q4_K_M)" })).toBe(
      "Faster captions (LMT-60 0.6B, 480 MB)",
    );
    expect(modelTitle({ id: "vad", name: "Voice detection" })).toBe("Voice detection");
    const model = (id: string, state: "ready" | "missing", optional: boolean) => ({
      id, name: id, bytes_total: 1, bytes_done: 1, state, optional, recommended: optional,
    });
    expect(modelsAllReady([model("a", "ready", false), model("draft", "missing", true)])).toBe(true);
    expect(modelsAllReady([model("a", "missing", false), model("draft", "ready", true)])).toBe(false);
    expect(modelsAllReady([model("draft", "ready", true)])).toBe(false);
    expect(modelsAllReady(null)).toBe(false);
  });

  it("builds the one-line footer", () => {
    expect(footerText("listening", STATS)).toBe("speech→English 1.8 s median · queue 1 · CPU 41%");
    expect(footerText("listening", { ...STATS, done_p50_ms: null })).toBe("speech→English — median · queue 1 · CPU 41%");
    expect(footerText("paused", STATS)).toBe("paused");
    expect(footerText("listening", null)).toBe("speech→English — median · queue — · CPU —");
  });

  it("computes model progress", () => {
    expect(modelsPercent({ bytes_done: 152, bytes_total: 239 })).toBe(64);
    expect(modelsPercent({ bytes_done: 5, bytes_total: 0 })).toBe(0);
    expect(modelsPercent({ bytes_done: 500, bytes_total: 100 })).toBe(100);
    const row = { id: "a", name: "A", bytes_total: 1, bytes_done: 1, state: "ready" as const, optional: false, recommended: false };
    expect(modelsAllReady([row])).toBe(true);
    expect(modelsAllReady([row, { ...row, state: "corrupt" }])).toBe(false);
    expect(modelsAllReady([])).toBe(false);
    expect(modelsAllReady(null)).toBe(false);
  });
});
