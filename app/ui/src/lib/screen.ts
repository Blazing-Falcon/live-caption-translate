import { get, writable, type Readable } from "svelte/store";
import * as api from "./api";
import { EVENTS, subscribe, type Disposer } from "./events";
import { formatBytes, modelsPercent } from "./format";
import type { ModelStatus, Rect, Rgb, ScreenBlock, ScreenEvent, ScreenFrame } from "./types";

/** The text recognizer in `models/manifest.json`. */
export const OCR_MODEL_ID = "pp-ocrv6-det-tiny-rec-small";
/** A drag smaller than this, in CSS px and in either direction, is a click and not a selection. */
export const MIN_SELECTION = 8;
export const MIN_FONT = 10;
/** Share of the Chinese line height that a translated box starts with as its font size. */
const FONT_SHARE = 0.8;
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

export const startFontPx = (block: ScreenBlock, scale: number): number => toCssPx(block.line_height, scale) * FONT_SHARE;

/**
 * Sets the largest font size from `start` down to `MIN_FONT` at which the text fits the box. A word that
 * is still wider than the box at `MIN_FONT` widens it, and a box that is still too tall grows downward.
 */
export function fitText(node: HTMLElement, width: number, height: number, start: number): void {
  const overflows = (): boolean => node.scrollHeight > node.clientHeight || node.scrollWidth > node.clientWidth;
  node.style.width = `${width}px`;
  node.style.height = `${height}px`;
  node.style.minHeight = "";
  let size = start;
  node.style.fontSize = `${size}px`;
  while (overflows() && size > MIN_FONT) {
    size = Math.max(MIN_FONT, size - 1);
    node.style.fontSize = `${size}px`;
  }
  if (node.scrollWidth > node.clientWidth) node.style.width = `${node.scrollWidth}px`;
  if (node.scrollHeight > node.clientHeight) {
    node.style.height = "auto";
    node.style.minHeight = `${height}px`;
  }
}

// Job state.

export type Outcome = { state: "translated"; text: string } | { state: "failed"; message: string };
export type Phase = "idle" | "selecting" | "reading" | "translating" | "done" | "error";

export interface ScreenState {
  frame: ScreenFrame | null;
  /** The last committed selection, CSS px. */
  selection: Rect | null;
  /** Id of the job whose events are shown; 0 until the first of its events or `screen_select` names it. */
  job: number;
  phase: Phase;
  blocks: ScreenBlock[];
  outcomes: Record<number, Outcome>;
  message: string | null;
  /** The text recognizer, when the job failed for lack of it. */
  model: ModelStatus | null;
  downloadError: string | null;
  /** The result layer is hidden. */
  original: boolean;
}

const IDLE: ScreenState = {
  frame: null,
  selection: null,
  job: 0,
  phase: "idle",
  blocks: [],
  outcomes: {},
  message: null,
  model: null,
  downloadError: null,
  original: false,
};

const withoutResults = (state: ScreenState): ScreenState => ({
  ...state,
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
      return { ...state, phase: "translating", blocks: event.blocks, outcomes: {} };
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
    if (model.state === "downloading") return `Downloading the text recognition model… ${modelsPercent(model)}%`;
    if (model.state === "verifying") return "Checking the downloaded model…";
    return `Screen translation needs the text recognition model (${formatBytes(model.bytes_total)}).`;
  }
  switch (state.phase) {
    case "reading":
      return "Reading text…";
    case "translating":
    case "done": {
      const { total, answered, failed } = progress(state);
      if (total === 0) return state.blocks.length === 0 ? "No text found." : "No Chinese text found.";
      if (state.phase === "translating") return `Translating ${answered} of ${total}…`;
      return failed > 0 ? `Done. ${failed} of ${total} could not be translated.` : "Done.";
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
  state: Readable<ScreenState>;
  /** Starts a job on `css` (CSS px). False when the drag is too small to be a selection. */
  select(css: Rect): boolean;
  /** Closes the window: stops the job and hides it. */
  close(): void;
  /** Forgets the frame and the job, without telling the backend (the window is already hidden). */
  reset(): void;
  setOriginal(on: boolean): void;
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

  function start(css: Rect, frame: ScreenFrame): void {
    drag += 1;
    const mine = drag;
    floor = highest;
    state.update((current) => ({ ...withoutResults(current), selection: css, job: 0, phase: "reading" }));
    api.screenSelect(frame.id, toFrameRect(css, frame)).then(
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
    floor = highest;
    state.set(IDLE);
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
    state.set({ ...IDLE, frame, phase: "selecting" });
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
    subscribe<ScreenFrame>(EVENTS.screenFrame, onFrame),
    subscribe<ScreenEvent>(EVENTS.screenEvent, onEvent),
    subscribe<ModelStatus[]>(EVENTS.modelsProgress, onProgress),
    subscribe<{ message: string }>(EVENTS.modelsError, ({ message }) =>
      state.update((current) => (current.model ? { ...current, downloadError: message } : current)),
    ),
  );

  return {
    state: { subscribe: state.subscribe },
    select,
    close,
    reset,
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
      for (const dispose of disposers.splice(0)) dispose();
    },
  };
}
