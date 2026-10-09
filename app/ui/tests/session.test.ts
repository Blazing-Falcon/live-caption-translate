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
  { id: "vad", name: "Voice detection", bytes_total: 2_000_000, bytes_done: 2_000_000, optional: false, recommended: false, state: "ready" },
  { id: "asr", name: "Speech recognition", bytes_total: 239_000_000, bytes_done: 0, optional: false, recommended: false, state: "missing" },
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



describe("save and act", () => {

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

});

