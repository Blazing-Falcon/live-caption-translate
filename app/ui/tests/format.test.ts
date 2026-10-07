import { describe, expect, it } from "vitest";
import {
  footerText,
  formatBytes,
  formatMemory,
  formatSeconds,
  levelText,
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

  it("sums CPU and memory across app and translator", () => {
    expect(totalCpu(STATS)).toBe(41);
    expect(totalCpu({ cpu_app_pct: 80, cpu_translator_pct: 150 })).toBe(230);
    expect(totalMemoryMb(STATS)).toBe(1900);
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
    const row = { id: "a", name: "A", bytes_total: 1, bytes_done: 1, state: "ready" as const };
    expect(modelsAllReady([row])).toBe(true);
    expect(modelsAllReady([row, { ...row, state: "corrupt" }])).toBe(false);
    expect(modelsAllReady([])).toBe(false);
    expect(modelsAllReady(null)).toBe(false);
  });
});
