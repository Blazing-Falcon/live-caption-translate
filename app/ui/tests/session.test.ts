// @vitest-environment jsdom
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { get } from "svelte/store";
import { afterEach, describe, expect, it } from "vitest";
import { createSession, applyEventToState, EVENTS, type Session } from "../src/lib/events";
import { DEFAULT_CONFIG, type SetConfigResult } from "../src/lib/settings";
import type { ModelStatus } from "../src/lib/types";
import { FakeClock, STATS, asrFinal, final, testConfig, testState } from "./helpers";

const MODELS: ModelStatus[] = [
  { id: "vad", name: "Voice detection", bytes_total: 2_000_000, bytes_done: 2_000_000, state: "ready" },
  { id: "asr", name: "Speech recognition", bytes_total: 239_000_000, bytes_done: 0, state: "missing" },
];

interface Backend {
  calls: { cmd: string; args: unknown }[];
  state: ReturnType<typeof testState>;
  setConfig: (patch: unknown) => SetConfigResult | Error;
  gate?: Promise<void>;
  startup?: string[];
}

function install(overrides: Partial<Backend> = {}): Backend {
  const backend: Backend = {
    calls: [],
    state: testState(),
    setConfig: () => ({ config: testConfig(), applied: "live", messages: [] }),
    ...overrides,
  };
  mockIPC(
    async (cmd, args) => {
      backend.calls.push({ cmd, args });
      if (backend.gate && (cmd === "get_state" || cmd === "get_config")) await backend.gate;
      switch (cmd) {
        case "get_state":
          return backend.state;
        case "get_config":
          return testConfig();
        case "get_stats":
          return STATS;
        case "models_status":
          return MODELS;
        case "startup_notices":
          return backend.startup ?? [];
        case "set_config": {
          const result = backend.setConfig((args as { patch: unknown }).patch);
          if (result instanceof Error) throw result.message;
          return result;
        }
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );
  return backend;
}

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));
const sessions: Session[] = [];

function open(kind: "overlay" | "control", clock = new FakeClock()) {
  const session = createSession({ kind, clock, reducedMotion: () => false });
  sessions.push(session);
  return { session, clock };
}

afterEach(() => {
  sessions.splice(0).forEach((session) => session.dispose());
  clearMocks();
});

describe("session loading", () => {
  it("loads state, config, models and stats for the control window", async () => {
    install();
    const { session } = open("control");
    await session.ready;
    await flush();
    expect(get(session.appState)?.listening).toBe("listening");
    expect(get(session.config)?.overlay.font_px).toBe(26);
    expect(get(session.models)).toEqual(MODELS);
    expect(get(session.stats)).toEqual(STATS);
    expect(get(session.loadError)).toBeNull();
  });

  it("shows startup notices as persistent warnings in the control window only", async () => {
    install({ startup: ["This PC has 2 CPU cores; 4 or more are recommended. Captions may lag."] });
    const { session, clock } = open("control");
    await session.ready;
    await flush();
    expect(get(session.notices).map((n) => [n.kind, n.text])).toEqual([
      ["warning", "This PC has 2 CPU cores; 4 or more are recommended. Captions may lag."],
    ]);
    clock.advance(60_000);
    expect(get(session.notices)).toHaveLength(1);
    const overlay = open("overlay");
    await overlay.session.ready;
    expect(get(overlay.session.notices)).toEqual([]);
  });

  it("does not load models or stats for the overlay", async () => {
    const backend = install();
    const { session } = open("overlay");
    await session.ready;
    await flush();
    expect(backend.calls.map((call) => call.cmd).sort()).toEqual(["get_config", "get_state"]);
    expect(get(session.models)).toBeNull();
  });

  it("reports a load failure in plain words", async () => {
    mockIPC(() => {
      throw "The app is still starting.";
    });
    const { session } = open("overlay");
    await session.ready;
    expect(get(session.loadError)).toBe("The app is still starting.");
    expect(get(session.config)).toBeNull();
  });

  it("replays events that arrive while state is loading", async () => {
    let release: () => void = () => undefined;
    install({ gate: new Promise<void>((resolve) => (release = resolve)) });
    const { session } = open("overlay");
    await flush();
    await emit(EVENTS.pipeline, asrFinal(1, "你好"));
    expect(get(session.captions).lines).toHaveLength(0);
    release();
    await session.ready;
    expect(get(session.captions).lines).toMatchObject([{ id: 1, state: "pending" }]);
  });
});

describe("session events", () => {
  it("feeds pipeline events to the caption store, stats store and app state", async () => {
    install();
    const { session } = open("control");
    await session.ready;
    await flush();
    await emit(EVENTS.pipeline, asrFinal(1, "你好"));
    await emit(EVENTS.pipeline, final(1, "Hello."));
    await emit(EVENTS.pipeline, { type: "stats", ...STATS, queue_depth: 4 });
    await emit(EVENTS.pipeline, { type: "listening_state", state: "paused" });
    await emit(EVENTS.pipeline, { type: "engine_status", engine: "asr", state: "failed", message: null });
    expect(get(session.captions).lines).toMatchObject([{ id: 1, state: "final", english: "Hello." }]);
    expect(get(session.stats)?.queue_depth).toBe(4);
    expect(get(session.stats)).not.toHaveProperty("type");
    expect(get(session.appState)).toMatchObject({ listening: "paused", engines: { asr: "failed" } });
    expect(get(session.captions).status?.kind).toBe("engine_failed");
  });

  it("applies config, overlay and hotkey events", async () => {
    install();
    const { session } = open("control");
    await session.ready;
    await flush();
    const next = testConfig((c) => (c.overlay.font_px = 34));
    await emit(EVENTS.configChanged, next);
    await emit(EVENTS.overlayMode, { moving: true });
    await emit(EVENTS.overlayVisible, { visible: false });
    await emit(EVENTS.overlayHover, { hover: true });
    await emit(EVENTS.modelsProgress, [{ ...MODELS[1], bytes_done: 100, state: "downloading" }]);
    await emit(EVENTS.hotkeysError, { action: "pause", accelerator: "Ctrl+Shift+P", message: "already in use" });
    expect(get(session.config)?.overlay.font_px).toBe(34);
    expect(get(session.appState)).toMatchObject({ overlay_moving: true, overlay_visible: false });
    expect(get(session.hover)).toBe(true);
    expect(get(session.models)).toHaveLength(1);
    expect(get(session.hotkeyErrors).pause?.message).toBe("already in use");
    session.clearHotkeyError("pause");
    expect(get(session.hotkeyErrors)).toEqual({});
  });

  it("ignores control-only events in the overlay window", async () => {
    install();
    const { session } = open("overlay");
    await session.ready;
    await flush();
    await emit(EVENTS.modelsProgress, MODELS);
    await emit(EVENTS.hotkeysError, { action: "pause", accelerator: "x", message: "m" });
    expect(get(session.models)).toBeNull();
    expect(get(session.hotkeyErrors)).toEqual({});
  });

  it("stops reacting after dispose and leaves no timers behind", async () => {
    install();
    const { session, clock } = open("overlay");
    await session.ready;
    await flush();
    await emit(EVENTS.pipeline, { type: "listening_state", state: "paused" });
    await emit(EVENTS.pipeline, asrFinal(1, "你好"));
    expect(get(session.captions).lines).toHaveLength(1);
    session.dispose();
    expect(clock.pending()).toBe(0);
    await emit(EVENTS.pipeline, asrFinal(2, "二"));
    await emit(EVENTS.configChanged, testConfig((c) => (c.overlay.font_px = 40)));
    expect(get(session.captions).lines).toHaveLength(0);
    expect(get(session.config)?.overlay.font_px).toBe(26);
    expect(clock.pending()).toBe(0);
  });
});

describe("applyEventToState", () => {
  it("tracks the header snapshot from events", () => {
    const base = testState({ models_ready: false, listening: "starting" });
    expect(applyEventToState(base, { type: "listening_state", state: "listening" })).toMatchObject({
      listening: "listening",
      models_ready: true,
    });
    expect(applyEventToState(base, { type: "source_state", state: "silent", detail: null }).source_state).toBe("silent");
    expect(applyEventToState(base, { type: "engine_status", engine: "vad", state: "loading", message: null }).engines.vad).toBe("loading");
    expect(applyEventToState(base, { type: "dropped", id: 1, reason: "empty" })).toBe(base);
  });
});

describe("save and act", () => {
  it("sends the patch, stores the returned config and surfaces the apply notices once", async () => {
    const returned = testConfig((c) => (c.overlay.font_px = 32));
    const backend = install({
      setConfig: () => ({ config: returned, applied: "restart", messages: ["Value clamped to 40"] }),
    });
    const { session } = open("control");
    await session.ready;
    const result = await session.save("overlay.font_px", { overlay: { font_px: 32 } });
    await session.save("overlay.font_px", { overlay: { font_px: 32 } });
    expect(result?.applied).toBe("restart");
    expect(get(session.config)).toEqual(returned);
    expect(backend.calls.filter((call) => call.cmd === "set_config")[0]?.args).toEqual({ patch: { overlay: { font_px: 32 } } });
    expect(get(session.notices).map((n) => n.text)).toEqual([
      "Value clamped to 40",
      "This change takes effect the next time the app starts.",
    ]);
  });

  it("mentions pipeline restarts and stays quiet for live and capture changes", async () => {
    let applied: SetConfigResult["applied"] = "pipeline";
    install({ setConfig: () => ({ config: testConfig(), applied, messages: [] }) });
    const { session } = open("control");
    await session.ready;
    await session.save("a", {});
    expect(get(session.notices)).toHaveLength(1);
    applied = "capture";
    session.dismissNotice(get(session.notices)[0]?.id ?? -1);
    await session.save("b", {});
    applied = "live";
    await session.save("c", {});
    expect(get(session.notices)).toHaveLength(0);
  });

  it("returns null, keeps the old config and shows a readable error on failure", async () => {
    install({ setConfig: () => new Error("Could not save settings: disk is full.") });
    const { session } = open("control");
    await session.ready;
    const before = get(session.config);
    expect(await session.save("overlay.style", { overlay: { style: "panel" } })).toBeNull();
    expect(get(session.config)).toBe(before);
    expect(get(session.notices)).toEqual([
      expect.objectContaining({ kind: "error", text: "Could not save settings: disk is full." }),
    ]);
    expect(get(session.busy).size).toBe(0);
  });

  it("marks only the operation's key busy while it runs", async () => {
    install();
    const { session } = open("control");
    await session.ready;
    let release: () => void = () => undefined;
    const running = session.act("run", () => new Promise<void>((resolve) => (release = resolve)));
    expect([...get(session.busy)]).toEqual(["run"]);
    release();
    await running;
    expect(get(session.busy).size).toBe(0);
  });

  it("expires info notices on the injected clock and keeps errors", async () => {
    install();
    const { session, clock } = open("control");
    await session.ready;
    session.pushNotice("info", "Saved");
    session.pushNotice("error", "Broken");
    clock.advance(8000);
    expect(get(session.notices).map((n) => n.text)).toEqual(["Broken"]);
  });
});

describe("defaults", () => {
  it("uses the shared default config shape in fixtures", () => {
    expect(testConfig()).toEqual(DEFAULT_CONFIG);
  });
});
