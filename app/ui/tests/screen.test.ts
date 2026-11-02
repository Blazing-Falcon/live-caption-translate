// @vitest-environment jsdom
import { emit } from "@tauri-apps/api/event";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { get } from "svelte/store";
import { afterEach, describe, expect, it } from "vitest";
import fixture from "../src/lib/__fixtures__/screen-events.json";
import { EVENTS } from "../src/lib/events";
import { DEFAULT_CONFIG } from "../src/lib/settings";
import {
  OCR_MODEL_ID,
  copyText,
  createScreen,
  placeBar,
  statusText,
  type Screen,
} from "../src/lib/screen";
import type { ModelStatus, Rect, ScreenEvent, ScreenFrame } from "../src/lib/types";

const events = fixture as unknown as ScreenEvent[];
const [reading, blocks, translated, failed, done, error] = events as [
  Extract<ScreenEvent, { type: "reading" }>,
  Extract<ScreenEvent, { type: "blocks" }>,
  Extract<ScreenEvent, { type: "translated" }>,
  Extract<ScreenEvent, { type: "failed" }>,
  Extract<ScreenEvent, { type: "done" }>,
  Extract<ScreenEvent, { type: "error" }>,
];
const JOB = reading.job;

const FRAME: ScreenFrame = { id: 1, width: 1200, height: 720, scale: 1.5 };
const AREA: Rect = { x: 40, y: 30, w: 400, h: 300 };
const MODEL: ModelStatus = {
  id: OCR_MODEL_ID,
  name: "Screen text recognition",
  bytes_total: 24_000_000,
  bytes_done: 0,
  optional: true,
  state: "missing",
};

interface Backend {
  direction: "horizontal" | "vertical";
  preferencesGate?: Promise<void>;
  calls: { cmd: string; args: Record<string, unknown> }[];
  /** Job ids `screen_select` answers with, in order. */
  jobs: number[];
  models: ModelStatus[];
  gate?: Promise<void>;
}

function install(overrides: Partial<Backend> = {}): Backend {
  const backend: Backend = { direction: "horizontal", calls: [], jobs: [JOB, JOB + 1, JOB + 2], models: [], ...overrides };
  mockIPC(
    async (cmd, args) => {
      backend.calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
      switch (cmd) {
        case "get_config":
          if (backend.preferencesGate) await backend.preferencesGate;
          return { ...DEFAULT_CONFIG, screen: { direction: backend.direction } };
        case "set_config": {
          const patch = (args as { patch: { screen: { direction: "horizontal" | "vertical" } } }).patch;
          backend.direction = patch.screen.direction;
          return { config: { ...DEFAULT_CONFIG, screen: { direction: backend.direction } }, applied: "live", messages: [] };
        }
        case "screen_select":
          if (backend.gate) await backend.gate;
          return backend.jobs.shift();
        case "models_status":
          return backend.models;
        default:
          return null;
      }
    },
    { shouldMockEvents: true },
  );
  return backend;
}

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));
const screens: Screen[] = [];

async function open(): Promise<Screen> {
  const screen = createScreen();
  screens.push(screen);
  await flush();
  await emit(EVENTS.screenFrame, FRAME);
  await flush();
  return screen;
}

async function send(...list: ScreenEvent[]): Promise<void> {
  for (const event of list) await emit(EVENTS.screenEvent, event);
  await flush();
}

const view = (screen: Screen) => get(screen.state);
const selects = (backend: Backend) => backend.calls.filter((call) => call.cmd === "screen_select");

it("uses the remembered direction on the first drag and keeps it across frames", async () => {
  const backend = install({ direction: "vertical" });
  const screen = await open();
  expect(view(screen).direction).toBe("vertical");
  screen.select(AREA);
  await flush();
  expect(selects(backend)[0]?.args.direction).toBe("vertical");
  screen.reset();
  await emit(EVENTS.screenFrame, { ...FRAME, id: 2 });
  await flush();
  expect(view(screen).direction).toBe("vertical");
});

it("saves a direction change and reads the existing selection again", async () => {
  const backend = install();
  const screen = await open();
  screen.select(AREA);
  await flush();
  await send(reading, blocks, translated, failed, done);
  await screen.setDirection("vertical");
  await flush();
  expect(backend.direction).toBe("vertical");
  expect(backend.calls.find(call => call.cmd === "set_config")?.args).toEqual({ patch: { screen: { direction: "vertical" } } });
  expect(selects(backend)).toHaveLength(2);
  expect(selects(backend)[1]?.args).toEqual({ ...selects(backend)[0]?.args, direction: "vertical" });
  expect(view(screen).blocks).toEqual([]);
});

it("does not start a drag after closing while saved preferences are loading", async () => {
  let release!: () => void;
  const backend = install({ preferencesGate: new Promise<void>(resolve => { release = resolve; }) });
  const screen = await open();
  screen.select(AREA);
  screen.close();
  release();
  await flush();
  expect(selects(backend)).toHaveLength(0);
});

afterEach(() => {
  screens.splice(0).forEach((screen) => screen.dispose());
  clearMocks();
});

describe("wire fixture", () => {
  const FIELDS: Record<ScreenEvent["type"], string[]> = {
    reading: ["job", "type"],
    blocks: ["blocks", "job", "ocr_lines", "ocr_ms", "type"],
    translated: ["id", "job", "text", "type"],
    failed: ["id", "job", "message", "type"],
    done: ["job", "total_ms", "type"],
    error: ["job", "message", "type"],
  };

  it("has one of each event with the fields the Rust side sends", () => {
    expect(events.map((event) => event.type)).toEqual(Object.keys(FIELDS));
    for (const event of events) expect(Object.keys(event).sort()).toEqual(FIELDS[event.type]);
  });

  it("has blocks in the shape of the Block type", () => {
    for (const block of blocks.blocks) {
      expect(Object.keys(block).sort()).toEqual(
        ["background", "foreground", "id", "intent", "line_height", "lines", "paint", "rect", "source", "translate", ...(block.tilt ? ["tilt"] : []), ...(block.runs_across ? ["runs_across"] : [])].sort(),
      );
      expect(Object.keys(block.rect).sort()).toEqual(["h", "w", "x", "y"]);
      for (const line of block.lines) expect(Object.keys(line).sort()).toEqual(["h", "w", "x", "y"]);
      for (const color of [block.background, block.foreground]) {
        expect(color).toHaveLength(3);
        for (const channel of color) expect(Number.isInteger(channel) && channel >= 0 && channel <= 255).toBe(true);
      }
    }
  });
});

it("requests raw lines when debug is enabled after a selection and keeps them on later toggles", async () => {
  const backend = install();
  const screen = await open();
  screen.select(AREA);
  await flush();
  expect(selects(backend)[0]!.args.debug).toBeUndefined();
  await send(reading, { ...blocks, ocr_lines: undefined }, done);
  screen.toggleDebug();
  await flush();
  expect(selects(backend)).toHaveLength(2);
  expect(selects(backend)[1]!.args).toMatchObject({ debug: true });
  expect(view(screen).blocks).toEqual([]);
  await send({ ...reading, job: JOB + 1 }, { ...blocks, job: JOB + 1 }, { ...done, job: JOB + 1 });
  expect(view(screen).ocrLines).toEqual(blocks.ocr_lines);
  screen.toggleDebug(); screen.toggleDebug();
  expect(selects(backend)).toHaveLength(2);
  screen.reset();
  expect(view(screen)).toMatchObject({ debug: false, ocrLines: null });
});

describe("a screen job", () => {
  it("follows the fixture from reading to done", async () => {
    install();
    const screen = await open();
    expect(screen.select(AREA)).toBe(true);
    await flush();
    expect(statusText(view(screen))).toBe("Reading text…");

    await send(reading, blocks);
    expect(view(screen).blocks.map((block) => block.id)).toEqual([0, 1, 2, 3]);
    expect(statusText(view(screen))).toBe("Translating 0 of 2…");

    await send(translated);
    expect(statusText(view(screen))).toBe("Translating 1 of 2…");
    await send(failed, done);
    expect(view(screen).outcomes).toEqual({
      0: { state: "translated", text: translated.text },
      2: { state: "failed", message: failed.message },
    });
    expect(statusText(view(screen))).toBe("1 of 2 bits couldn't be translated. Point at a red mark to see why.");
    expect(copyText(view(screen), "english")).toBe(translated.text);
    expect(copyText(view(screen), "chinese")).toBe(`${blocks.blocks[0]?.source}\n\n${blocks.blocks[2]?.source}`);
  });

  it("marks the blocks still waiting as failed when done comes first", async () => {
    install();
    const screen = await open();
    screen.select(AREA);
    await flush();
    await send(reading, blocks, translated, done);
    expect(view(screen).outcomes[2]?.state).toBe("failed");
    expect(view(screen).outcomes[0]?.state).toBe("translated");
  });

  it("ignores the events of an older job after a new drag", async () => {
    const backend = install();
    const screen = await open();
    screen.select(AREA);
    await flush();
    await send(reading, blocks);

    screen.select({ ...AREA, x: 200 });
    await flush();
    expect(selects(backend)).toHaveLength(2);
    expect(view(screen)).toMatchObject({ job: JOB + 1, blocks: [], phase: "reading" });

    await send(translated, failed, done, { ...blocks, blocks: [] });
    expect(view(screen)).toMatchObject({ job: JOB + 1, blocks: [], outcomes: {}, phase: "reading" });

    await send({ ...reading, job: JOB + 1 }, { ...blocks, job: JOB + 1 }, { ...translated, job: JOB + 1 });
    expect(view(screen).blocks).toHaveLength(4);
    expect(view(screen).outcomes[0]?.state).toBe("translated");
  });

  it("keeps events that arrive before screen_select names the job", async () => {
    let release: () => void = () => undefined;
    install({ gate: new Promise<void>((resolve) => (release = resolve)) });
    const screen = await open();
    screen.select(AREA);
    await send(reading, blocks);
    expect(view(screen)).toMatchObject({ job: JOB, phase: "translating" });
    release();
    await flush();
    expect(view(screen)).toMatchObject({ job: JOB, phase: "translating" });
    expect(view(screen).blocks).toHaveLength(4);
  });

  it("changes nothing for events before any drag", async () => {
    install();
    const screen = await open();
    await send(reading, blocks);
    expect(view(screen)).toMatchObject({ phase: "selecting", blocks: [] });
  });
});

describe("selection", () => {
  it("sends frame pixels, the CSS rect times the scale rounded outward", async () => {
    const backend = install();
    const screen = await open();
    screen.select({ x: 100.2, y: 50.1, w: 200.5, h: 80.3 });
    await flush();
    expect(selects(backend)[0]?.args).toEqual({ frame: FRAME.id, direction: "horizontal", rect: { x: 150, y: 75, w: 302, h: 121 } });
  });

  it("clamps to the frame and ignores a drag under 8 x 8 CSS px", async () => {
    const backend = install();
    const screen = await open();
    expect(screen.select({ x: 10, y: 10, w: 7.9, h: 200 })).toBe(false);
    expect(screen.select({ x: 10, y: 10, w: 200, h: 7.9 })).toBe(false);
    expect(screen.select({ x: 795, y: 475, w: 100, h: 100 })).toBe(false);
    expect(selects(backend)).toHaveLength(0);
    expect(view(screen).selection).toBeNull();

    expect(screen.select({ x: -50, y: -50, w: 900, h: 600 })).toBe(true);
    await flush();
    expect(selects(backend)[0]?.args).toEqual({ frame: FRAME.id, direction: "horizontal", rect: { x: 0, y: 0, w: 1200, h: 720 } });
  });

  it("puts the toolbar under the box, above it near the bottom edge, else inside the bottom", () => {
    const bar = { w: 300, h: 60 };
    const viewport = { w: 800, h: 480 };
    expect(placeBar({ x: 40, y: 30, w: 200, h: 100 }, bar, viewport)).toEqual({ left: 40, top: 138 });
    expect(placeBar({ x: 600, y: 300, w: 200, h: 170 }, bar, viewport)).toEqual({ left: 500, top: 232 });
    expect(placeBar({ x: 0, y: 20, w: 800, h: 460 }, bar, viewport)).toEqual({ left: 0, top: 412 });
  });
});

describe("a missing text recognition model", () => {
  it("offers the download, shows its progress and reads the area again when it is ready", async () => {
    const backend = install({ models: [MODEL] });
    const screen = await open();
    screen.select(AREA);
    await flush();
    await send(reading, { ...error, job: JOB });
    expect(view(screen).model?.state).toBe("missing");
    expect(statusText(view(screen))).toBe("To read screenshots, download the text reader (23 MB, once).");

    await screen.downloadModel();
    expect(backend.calls.filter((call) => call.cmd === "models_download_optional")).toEqual([
      { cmd: "models_download_optional", args: { id: OCR_MODEL_ID } },
    ]);
    await emit(EVENTS.modelsProgress, [{ ...MODEL, state: "downloading", bytes_done: 12_000_000 }]);
    await flush();
    expect(statusText(view(screen))).toBe("Downloading the text reader… 50%");

    await emit(EVENTS.modelsProgress, [{ ...MODEL, state: "ready", bytes_done: MODEL.bytes_total }]);
    await flush();
    expect(selects(backend)).toHaveLength(2);
    expect(view(screen)).toMatchObject({ model: null, phase: "reading" });
  });

  it("shows the message as it is when the model is there", async () => {
    install({ models: [{ ...MODEL, state: "ready", bytes_done: MODEL.bytes_total }] });
    const screen = await open();
    screen.select(AREA);
    await flush();
    await send(reading, { ...error, job: JOB });
    expect(view(screen).model).toBeNull();
    expect(statusText(view(screen))).toBe(error.message);
  });
});
