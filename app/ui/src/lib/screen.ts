import { get, writable, type Readable } from "svelte/store";
import * as api from "./api";
import { EVENTS, subscribe, type Disposer } from "./events";
import { formatBytes, modelsPercent } from "./format";
import type { Config } from "./settings";
import type { ModelStatus, Rect, RectF, Rgb, ScreenBlock, ScreenEvent, ScreenFrame, ScreenDirection } from "./types";

/** The text recognizer in `models/manifest.json`. */
export const OCR_MODEL_ID = "pp-ocrv6-det-tiny-rec-small";
/** A drag smaller than this, in CSS px and in either direction, is a click and not a selection. */
export const MIN_SELECTION = 8;
const BAR_GAP = 8;

export const frameUrl = (id: number): string => `http://ltshot.localhost/${id}`;

// Coordinates. A frame is physical pixels of the captured monitor; the page is CSS pixels;
// CSS px = frame px / scale. Nothing outside this section multiplies or divides by the scale.

const EPS = 1e-6;

export interface Size {
  w: number;
  h: number;
}

export function cssSize(frame: ScreenFrame): Size {
  return { w: frame.width / frame.scale, h: frame.height / frame.scale };
}

export function toCssRect(rect: Rect, scale: number): Rect {
  return { x: rect.x / scale, y: rect.y / scale, w: rect.w / scale, h: rect.h / scale };
}

export function toCssPx(framePx: number, scale: number): number {
  return framePx / scale;
}

export function toCssFloatRect(rect: RectF, scale: number): Rect {
  return { x: rect.x / scale, y: rect.y / scale, w: rect.width / scale, h: rect.height / scale };
}

export function toFrameFloatRect(rect: Rect, scale: number): RectF {
  return { x: rect.x * scale, y: rect.y * scale, width: rect.w * scale, height: rect.h * scale };
}

export function toFramePx(cssPx: number, scale: number): number {
  return cssPx * scale;
}

/** Frame pixels of a CSS rect, rounded outward so the whole selection is read. */
export function toFrameRect(css: Rect, frame: ScreenFrame): Rect {
  const left = Math.max(0, Math.floor(css.x * frame.scale + EPS));
  const top = Math.max(0, Math.floor(css.y * frame.scale + EPS));
  const right = Math.min(frame.width, Math.ceil((css.x + css.w) * frame.scale - EPS));
  const bottom = Math.min(frame.height, Math.ceil((css.y + css.h) * frame.scale - EPS));
  return { x: left, y: top, w: Math.max(0, right - left), h: Math.max(0, bottom - top) };
}

export function clampToFrame(css: Rect, frame: ScreenFrame): Rect {
  const size = cssSize(frame);
  const left = Math.min(Math.max(css.x, 0), size.w);
  const top = Math.min(Math.max(css.y, 0), size.h);
  const right = Math.min(Math.max(css.x + css.w, 0), size.w);
  const bottom = Math.min(Math.max(css.y + css.h, 0), size.h);
  return { x: left, y: top, w: right - left, h: bottom - top };
}

export function rectBetween(a: { x: number; y: number }, b: { x: number; y: number }): Rect {
  return { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), w: Math.abs(a.x - b.x), h: Math.abs(a.y - b.y) };
}

/** Where the toolbar goes: under the selection, above it when there is no room below, else at the bottom. */
export function placeBar(selection: Rect, bar: Size, viewport: Size): { left: number; top: number } {
  const left = Math.min(Math.max(selection.x, 0), Math.max(0, viewport.w - bar.w));
  const below = selection.y + selection.h + BAR_GAP;
  if (below + bar.h <= viewport.h) return { left, top: below };
  const above = selection.y - BAR_GAP - bar.h;
  if (above >= 0) return { left, top: above };
  return { left, top: Math.max(0, viewport.h - bar.h - BAR_GAP) };
}

export const cssColor = (color: Rgb): string => `rgb(${color[0]} ${color[1]} ${color[2]})`;

// Job state.

export type Outcome = { state: "translated"; text: string } | { state: "failed"; message: string };
export type Phase = "idle" | "selecting" | "reading" | "translating" | "done" | "error";

export interface ScreenState {
  direction: ScreenDirection;
  frame: ScreenFrame | null;
  /** The last committed selection, CSS px. */
  selection: Rect | null;
  /** Id of the job whose events are shown; 0 until the first of its events or `screen_select` names it. */
  job: number;
  phase: Phase;
  blocks: ScreenBlock[];
  ocrLines: Rect[] | null;
  debug: boolean;
  outcomes: Record<number, Outcome>;
  message: string | null;
  /** The text recognizer, when the job failed for lack of it. */
  model: ModelStatus | null;
  downloadError: string | null;
  /** The result layer is hidden. */
  original: boolean;
}

const IDLE: ScreenState = {
  direction: "horizontal",
  frame: null,
  selection: null,
  job: 0,
  phase: "idle",
  blocks: [],
  ocrLines: null,
  debug: false,
  outcomes: {},
  message: null,
  model: null,
  downloadError: null,
  original: false,
};

const withoutResults = (state: ScreenState): ScreenState => ({
  ...state,
  ocrLines: null,
  blocks: [],
  outcomes: {},
  message: null,
  model: null,
  downloadError: null,
});

export function applyEvent(state: ScreenState, event: ScreenEvent): ScreenState {
  switch (event.type) {
    case "reading":
      return { ...state, phase: "reading", message: null };
    case "blocks":
      return { ...state, phase: "translating", blocks: event.blocks, ocrLines: event.ocr_lines ?? null, outcomes: {} };
    case "translated":
    case "failed": {
      if (!state.blocks.some((block) => block.id === event.id && block.translate)) return state;
      const outcome: Outcome =
        event.type === "translated"
          ? { state: "translated", text: event.text }
          : { state: "failed", message: event.message };
      return { ...state, outcomes: { ...state.outcomes, [event.id]: outcome } };
    }
    case "done": {
      const outcomes = { ...state.outcomes };
      for (const block of state.blocks) {
        if (block.translate && !outcomes[block.id]) {
          outcomes[block.id] = { state: "failed", message: "The translation did not arrive." };
        }
      }
      return { ...state, phase: "done", outcomes };
    }
    case "error":
      return { ...state, phase: "error", message: event.message };
  }
}

export function progress(state: ScreenState): { total: number; answered: number; failed: number } {
  const wanted = state.blocks.filter((block) => block.translate);
  const outcomes = wanted.map((block) => state.outcomes[block.id]);
  return {
    total: wanted.length,
    answered: outcomes.filter(Boolean).length,
    failed: outcomes.filter((outcome) => outcome?.state === "failed").length,
  };
}

export function modelBusy(model: ModelStatus | null): boolean {
  return model?.state === "downloading" || model?.state === "verifying";
}

export function statusText(state: ScreenState): string {
  const { model } = state;
  if (model && model.state !== "ready") {
    if (model.state === "downloading") return `Downloading the text reader… ${modelsPercent(model)}%`;
    if (model.state === "verifying") return "Checking the download…";
    return `To read screenshots, download the text reader (${formatBytes(model.bytes_total)}, once).`;
  }
  switch (state.phase) {
    case "reading":
      return "Reading text…";
    case "translating":
    case "done": {
      const { total, answered, failed } = progress(state);
      if (total === 0) return state.blocks.length === 0 ? "No text found." : "No Chinese text found.";
      if (state.phase === "translating") return `Translating ${answered} of ${total}…`;
      if (failed > 0) return `${failed} of ${total} ${total === 1 ? "bit" : "bits"} couldn't be translated. Point at a red mark to see why.`;
      return `Done. ${total} ${total === 1 ? "bit" : "bits"} translated.`;
    }
    case "error":
      return state.message ?? "Something went wrong. Please try again.";
    default:
      return "";
  }
}

/** Blocks in reading order, separated by blank lines. English covers the translated blocks only. */
export function copyText(state: ScreenState, kind: "english" | "chinese"): string {
  const parts: string[] = [];
  for (const block of state.blocks) {
    if (!block.translate) continue;
    if (kind === "chinese") parts.push(block.source);
    else {
      const outcome = state.outcomes[block.id];
      if (outcome?.state === "translated") parts.push(outcome.text);
    }
  }
  return parts.join("\n\n");
}

export interface Screen {
  setDirection(direction: ScreenDirection): Promise<void>;
  state: Readable<ScreenState>;
  /** Starts a job on `css` (CSS px). False when the drag is too small to be a selection. */
  select(css: Rect): boolean;
  /** Closes the window: stops the job and hides it. */
  close(): void;
  /** Forgets the frame and the job, without telling the backend (the window is already hidden). */
  reset(): void;
  setOriginal(on: boolean): void;
  toggleDebug(): void;
  downloadModel(): Promise<void>;
  copy(kind: "english" | "chinese"): Promise<boolean>;
  dispose(): void;
}

export function createScreen(): Screen {
  const state = writable<ScreenState>(IDLE);
  const disposers: Disposer[] = [];
  /** One for each job this page started. */
  let drag = 0;
  /** The largest job id seen so far. */
  let highest = 0;
  /** Events of jobs at or below this id belong to an older drag or frame and change nothing. */
  let floor = 0;
  let directionChange = 0;
  let savingDirection = false;
  let disposed = false;

  function applyDirection(direction: ScreenDirection): void {
    const current = get(state);
    if (disposed || current.direction === direction) return;
    state.update(now => ({ ...now, direction }));
    if (current.frame && current.selection) start(current.selection, current.frame);
  }
  const ready = api.getConfig().then(config => applyDirection(config.screen.direction)).catch(() => undefined);

  function start(css: Rect, frame: ScreenFrame): void {
    drag += 1;
    const mine = drag;
    floor = highest;
    state.update((current) => ({ ...withoutResults(current), selection: css, job: 0, phase: "reading" }));
    void ready.then(() => {
      if (disposed || mine !== drag || get(state).frame?.id !== frame.id) return;
      return api.screenSelect(frame.id, toFrameRect(css, frame), get(state).debug, get(state).direction).then(
      (id) => {
        highest = Math.max(highest, id);
        if (mine !== drag) {
          floor = Math.max(floor, id);
          return;
        }
        state.update((current) => (id > current.job ? { ...withoutResults(current), job: id, phase: "reading" } : current));
      },
      (error: unknown) => {
        if (mine !== drag) return;
        state.update((current) => ({ ...current, phase: "error", message: api.readableError(error) }));
      },
      );
    });
  }

  function select(css: Rect): boolean {
    const { frame } = get(state);
    if (!frame) return false;
    const rect = clampToFrame(css, frame);
    if (rect.w < MIN_SELECTION || rect.h < MIN_SELECTION) return false;
    start(rect, frame);
    return true;
  }

  function reset(): void {
    drag += 1;
    floor = highest;
    state.set({ ...IDLE, direction: get(state).direction });
  }

  function close(): void {
    reset();
    api.screenClose().catch(() => undefined);
  }

  async function checkModel(job: number): Promise<void> {
    let row: ModelStatus | undefined;
    try {
      row = (await api.modelsStatus()).find((model) => model.id === OCR_MODEL_ID);
    } catch {
      return;
    }
    if (!row || row.state === "ready") return;
    const found = row;
    state.update((current) =>
      current.job === job && current.phase === "error" ? { ...current, model: found } : current,
    );
  }

  function onFrame(frame: ScreenFrame): void {
    floor = highest;
    state.set({ ...IDLE, frame, phase: "selecting", direction: get(state).direction });
  }

  function onEvent(event: ScreenEvent): void {
    highest = Math.max(highest, event.job);
    const current = get(state);
    // A job's first events can arrive before `screen_select` names it.
    if (!current.frame || !current.selection || event.job <= floor || event.job < current.job) return;
    state.update((now) => {
      const base = event.job > now.job ? { ...withoutResults(now), job: event.job } : now;
      return applyEvent(base, event);
    });
    if (event.type === "error") void checkModel(event.job);
  }

  function onProgress(rows: ModelStatus[]): void {
    const row = rows.find((model) => model.id === OCR_MODEL_ID);
    const current = get(state);
    if (!row || !current.model) return;
    state.set({ ...current, model: row });
    if (row.state === "ready" && current.model.state !== "ready" && current.frame && current.selection) {
      start(current.selection, current.frame);
    }
  }

  disposers.push(
    subscribe<Config>(EVENTS.configChanged, config => { if (!savingDirection) applyDirection(config.screen.direction); }),
    subscribe<ScreenFrame>(EVENTS.screenFrame, onFrame),
    subscribe<ScreenEvent>(EVENTS.screenEvent, onEvent),
    subscribe<ModelStatus[]>(EVENTS.modelsProgress, onProgress),
    subscribe<{ message: string }>(EVENTS.modelsError, ({ message }) =>
      state.update((current) => (current.model ? { ...current, downloadError: message } : current)),
    ),
  );

  return {
    state: { subscribe: state.subscribe },
    async setDirection(direction) {
      await ready;
      const previous = get(state).direction;
      if (direction === previous) return;
      const change = ++directionChange;
      savingDirection = true;
      state.update(now => ({ ...now, direction }));
      try {
        await api.setConfig({ screen: { direction } });
        if (change !== directionChange || disposed) return;
        const current = get(state);
        if (current.frame && current.selection) start(current.selection, current.frame);
      } catch (error) {
        if (change === directionChange && !disposed) state.update(now => ({ ...now, direction: previous, phase: "error", message: api.readableError(error) }));
      } finally {
        if (change === directionChange) savingDirection = false;
      }
    },
    select,
    close,
    reset,
    toggleDebug() {
      const current = get(state), debug = !current.debug;
      state.update(now => ({ ...now, debug }));
      if (debug && current.ocrLines === null && current.frame && current.selection) start(current.selection, current.frame);
    },
    setOriginal: (on) => state.update((current) => (current.original === on ? current : { ...current, original: on })),
    async downloadModel() {
      const { model } = get(state);
      if (!model || modelBusy(model)) return;
      state.update((current) => ({ ...current, downloadError: null }));
      try {
        await api.modelsDownloadOptional(model.id);
      } catch (error) {
        state.update((current) => ({ ...current, downloadError: api.readableError(error) }));
      }
    },
    async copy(kind) {
      const text = copyText(get(state), kind);
      if (text === "") return false;
      try {
        await navigator.clipboard.writeText(text);
        return true;
      } catch {
        return false;
      }
    },
    dispose() {
      disposed = true;
      drag += 1;
      for (const dispose of disposers.splice(0)) dispose();
    },
  };
}
