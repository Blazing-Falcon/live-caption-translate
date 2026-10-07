import { describe, expect, it } from "vitest";
import { deriveStatus, pausedText, type StatusInput, type StatusKind } from "../src/lib/captions";

const BASE: StatusInput = {
  modelsReady: true,
  engines: { vad: "ready", asr: "ready", translator: "ready" },
  listening: "listening",
  sourceState: "playing",
  sourceDetail: null,
  selectedAppNames: [],
  pauseHotkey: "Ctrl+Shift+P",
};

const text = (patch: Partial<StatusInput>): string | null => deriveStatus({ ...BASE, ...patch })?.text ?? null;

describe("deriveStatus", () => {
  it("returns nothing for the healthy and silent states", () => {
    expect(deriveStatus(BASE)).toBeNull();
    expect(deriveStatus({ ...BASE, sourceState: "silent" })).toBeNull();
    expect(deriveStatus({ ...BASE, listening: "starting" })).toBeNull();
  });

  it.each([
    [{ modelsReady: false }, "Models not downloaded. Open settings to download."],
    [{ engines: { asr: "failed" } }, "Speech recognition stopped working. Open settings for details."],
    [{ engines: { vad: "failed" } }, "Speech recognition stopped working. Open settings for details."],
    [{ engines: { translator: "failed" } }, "Translator stopped working. Open settings for details."],
    [{ engines: { vad: "loading" } }, "Loading speech recognition…"],
    [{ engines: { asr: "loading" } }, "Loading speech recognition…"],
    [{ engines: { asr: "restarting" } }, "Loading speech recognition…"],
    [{ engines: { translator: "loading" } }, "Starting translator…"],
    [{ engines: { translator: "restarting" } }, "Restarting translator…"],
    [{ listening: "paused" }, "Paused · Ctrl Shift P to resume"],
    [{ sourceState: "no_device" }, "No audio device"],
    [{ sourceState: "no_apps_selected" }, "No apps selected"],
    [{ sourceState: "apps_not_running", sourceDetail: "Chrome, VLC" }, "Waiting for Chrome, VLC"],
    [{ sourceState: "apps_not_running", selectedAppNames: ["Discord"] }, "Waiting for Discord"],
    [{ sourceState: "apps_not_running" }, "Waiting for the selected apps"],
    [{ sourceState: "unsupported" }, "Selected apps needs Windows 11. Listening to the whole system."],
  ] as [Partial<StatusInput>, string][])("single condition %j", (patch, expected) => {
    expect(text(patch)).toBe(expected);
  });

  it("resolves every combination of overlapping conditions by the documented priority", () => {
    const conditions: { kind: StatusKind; patch: Partial<StatusInput> }[] = [
      { kind: "models_missing", patch: { modelsReady: false } },
      { kind: "engine_failed", patch: { engines: { vad: "ready", asr: "failed", translator: "ready" } } },
      { kind: "engine_loading", patch: { engines: { vad: "ready", asr: "ready", translator: "loading" } } },
      { kind: "paused", patch: { listening: "paused" } },
      { kind: "no_device", patch: { sourceState: "no_device" } },
    ];
    for (let mask = 0; mask < 1 << conditions.length; mask += 1) {
      const active = conditions.filter((_, index) => (mask & (1 << index)) !== 0);
      let input: StatusInput = { ...BASE };
      for (const condition of active) input = { ...input, ...condition.patch };
      if (active.some((c) => c.kind === "engine_failed") && active.some((c) => c.kind === "engine_loading")) {
        input = { ...input, engines: { vad: "ready", asr: "failed", translator: "loading" } };
      }
      expect(deriveStatus(input)?.kind ?? null).toBe(active[0]?.kind ?? null);
    }
  });

  it("checks all failed engines before any loading engine, in vad/asr/translator order", () => {
    expect(text({ engines: { vad: "loading", asr: "ready", translator: "failed" } })).toBe(
      "Translator stopped working. Open settings for details.",
    );
    expect(text({ engines: { vad: "failed", asr: "ready", translator: "failed" } })).toBe(
      "Speech recognition stopped working. Open settings for details.",
    );
    expect(text({ engines: { vad: "loading", asr: "ready", translator: "restarting" } })).toBe(
      "Loading speech recognition…",
    );
  });

  it("lets paused beat source conditions and lets source conditions apply when listening", () => {
    expect(text({ listening: "paused", sourceState: "no_device" })).toBe("Paused · Ctrl Shift P to resume");
    expect(text({ listening: "listening", sourceState: "no_apps_selected" })).toBe("No apps selected");
  });

  it("prints the actual pause binding and never an empty shortcut", () => {
    expect(pausedText("Ctrl+Alt+Space")).toBe("Paused · Ctrl Alt Space to resume");
    expect(pausedText("")).toBe("Paused · Resume in Settings");
    expect(pausedText("  ")).toBe("Paused · Resume in Settings");
  });
});
