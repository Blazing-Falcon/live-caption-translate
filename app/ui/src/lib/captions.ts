import { writable, type Readable } from "svelte/store";
import { formatAccelerator, type Config } from "./settings";
import type {
  AppState,
  CaptionLine,
  EngineKind,
  EngineState,
  FailReason,
  ListeningStateKind,
  PipelineEvent,
  SourceInfo,
  SourceStateKind,
  UtteranceId,
} from "./types";

export const CHIP_MS = 3000;
/** Terminal rows kept after they leave the screen: three times the 10 a style can show, so a
 * short line does not push a full screen out. */
export const MAX_HISTORY_LINES = 30;
/** Defensive cap: the core bounds outstanding work, the UI must not trust that blindly. */
export const MAX_OUTSTANDING_LINES = 32;

export interface Clock {
  now(): number;
  setTimer(fn: () => void, ms: number): unknown;
  clearTimer(handle: unknown): void;
}

export const systemClock: Clock = {
  now: () => performance.now(),
  setTimer: (fn, ms) => setTimeout(fn, ms),
  clearTimer: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export type StatusKind =
  | "models_missing"
  | "engine_failed"
  | "engine_loading"
  | "paused"
  | "no_device"
  | "no_apps"
  | "apps_not_running"
  | "unsupported";

export interface StatusLine {
  kind: StatusKind;
  text: string;
}

export interface ChipMessage {
  id: number;
  text: string;
}

/** Running text: the English of the lines not yet translated for good, `id` first of `ids`. */
export interface Preview {
  id: UtteranceId;
  ids: readonly UtteranceId[];
  text: string;
}

/** A unit the core sent for final translation: its first line, then every line it covers. */
export interface Seal {
  id: UtteranceId;
  ids: readonly UtteranceId[];
}

export type CaptionState = "open" | "pending" | "streaming" | "final" | "failed" | "skipped" | "english";

/** One row group of the overlay. A sealed unit keeps its key when the core seals it again with
 * more lines, so the row the reader is watching stays the same element. */
export interface CaptionBlock {
  key: UtteranceId;
  ids: readonly UtteranceId[];
  /** A unit the core sent for final translation; otherwise the open run of lines after it. */
  sealed: boolean;
  source: string;
  /** Chinese of the open block's lines still being heard. */
  live: string;
  english: string;
  state: CaptionState;
  reason: string | null;
  /** English that the next translation replaces. */
  preview: string | null;
}

export interface CaptionView {
  blocks: readonly CaptionBlock[];
  status: StatusLine | null;
  chip: ChipMessage | null;
  announcement: string;
}

export interface StatusInput {
  modelsReady: boolean;
  engines: Partial<Record<EngineKind, EngineState>>;
  listening: ListeningStateKind;
  sourceState: SourceStateKind;
  sourceDetail: string | null;
  selectedAppNames: readonly string[];
  pauseHotkey: string;
  settingsHotkey: string;
}

const ENGINE_ORDER: readonly EngineKind[] = ["asr", "translator"];

export function isTerminal(state: CaptionLine["state"]): boolean {
  return state !== "live" && state !== "pending" && state !== "streaming";
}

export function failReasonText(reason: FailReason, message: string): string {
  switch (reason) {
    case "timeout":
      return "Translation took too long";
    case "server_unavailable":
      return "Translator is restarting";
    case "echo":
    case "runaway":
      return "Could not translate this line";
    case "error":
      return message.trim() || "Could not translate this line";
  }
}

/** The overlay lets clicks through, so a status can only name a way to open settings. The
 * shortcut is named first, and the tray is always named too: a shortcut that another program
 * owns fails to register, and the overlay has no way to learn that. */
function openSettingsText(settingsHotkey: string): string {
  const binding = formatAccelerator(settingsHotkey);
  return binding === ""
    ? "Open settings from the tray"
    : `Open settings with ${binding} or from the tray`;
}

export function pausedText(pauseHotkey: string): string {
  const binding = formatAccelerator(pauseHotkey);
  return binding === "" ? "Paused · Resume in Settings" : `Paused · ${binding} to resume`;
}

/** Priority order is fixed; silence is handled by the chip. */
export function deriveStatus(input: StatusInput): StatusLine | null {
  if (!input.modelsReady) {
    return {
      kind: "models_missing",
      text: `Models not downloaded. ${openSettingsText(input.settingsHotkey)}`,
    };
  }
  for (const engine of ENGINE_ORDER) {
    if (input.engines[engine] === "failed") {
      const name = engine === "translator" ? "Translator" : "Speech recognition";
      return {
        kind: "engine_failed",
        text: `${name} stopped working. ${openSettingsText(input.settingsHotkey)} for details.`,
      };
    }
  }
  for (const engine of ENGINE_ORDER) {
    const state = input.engines[engine];
    if (state === "loading" || state === "restarting") {
      if (engine === "translator") {
        return {
          kind: "engine_loading",
          text: state === "restarting" ? "Restarting translator…" : "Starting translator…",
        };
      }
      return { kind: "engine_loading", text: "Loading speech recognition…" };
    }
  }
  if (input.listening === "paused") return { kind: "paused", text: pausedText(input.pauseHotkey) };
  switch (input.sourceState) {
    case "no_device":
      return { kind: "no_device", text: "No audio device" };
    case "no_apps_selected":
      return { kind: "no_apps", text: "No apps selected" };
    case "apps_not_running": {
      const names = input.sourceDetail?.trim() || input.selectedAppNames.join(", ");
      return { kind: "apps_not_running", text: names ? `Waiting for ${names}` : "Waiting for the selected apps" };
    }
    case "unsupported":
      return {
        kind: "unsupported",
        text: "Selected apps needs Windows 11. Listening to the whole system.",
      };
    case "playing":
    case "silent":
      return null;
  }
}

export function silentChipText(source: SourceInfo | null): string {
  const label = source?.label.trim();
  return label ? `No audio playing on ${label}` : "No audio playing";
}


/** Newest line is full strength; older lines fade by index and stop at the last step. The
 * dimmest step stays at 0.6: below that the English drops under 4.5:1 against white video. */
const AGE_OPACITY: readonly number[] = [1, 0.75, 0.6];

export function ageOpacity(age: number): number {
  return AGE_OPACITY[Math.min(Math.max(age, 0), AGE_OPACITY.length - 1)] ?? 1;
}

/** A sealed line waits for its translation, so its block is pending even if it was still live. */
function blockState(state: CaptionLine["state"]): CaptionState {
  return state === "live" ? "pending" : state;
}

/** The blocks the overlay shows, oldest first: one for each sealed unit and one open block for
 * the lines the core has not sealed yet. Grouping is by seal, so a row never splits or merges
 * when a preview arrives or a unit is sealed. */
export function captionBlocks(
  lines: readonly CaptionLine[],
  seals: ReadonlyMap<UtteranceId, Seal>,
  preview: Preview | null,
  liveSource: boolean,
): CaptionBlock[] {
  const byId = new Map(lines.map((line) => [line.id, line]));
  const sealOf = new Map<UtteranceId, Seal>();
  for (const seal of seals.values()) {
    for (const id of seal.ids) sealOf.set(id, seal);
  }

  const blocks: CaptionBlock[] = [];
  const done = new Set<UtteranceId>();
  let open: CaptionLine[] = [];
  let previewShown = false;

  const previewFor = (ids: readonly UtteranceId[]): string | null => {
    if (previewShown || preview === null || !ids.some((id) => preview.ids.includes(id))) return null;
    previewShown = true;
    return preview.text;
  };

  const emit = (key: UtteranceId, ids: readonly UtteranceId[], leader: CaptionLine): void => {
    const english = leader.state === "english";
    blocks.push({
      key,
      ids,
      sealed: true,
      source: leader.source,
      live: "",
      english: english ? "" : leader.english,
      state: blockState(leader.state),
      reason: leader.reason,
      preview: english ? null : previewFor(ids),
    });
  };

  const flushOpen = (): void => {
    const first = open[0];
    const run = open;
    open = [];
    if (first === undefined) return;
    const ids = run.map((line) => line.id);
    const live = run.filter((line) => line.state === "live");
    const source = run
      .filter((line) => line.state !== "live")
      .map((line) => line.source)
      .join("");
    const liveText = liveSource ? live.map((line) => line.source).join("") : "";
    const preview = previewFor(ids);
    const english = live.length === run.length ? "" : first.english;
    // A live line whose Chinese is hidden has nothing to show, so it takes no block and no space.
    if (source === "" && liveText === "" && english === "" && preview === null) return;
    blocks.push({
      key: first.id,
      ids,
      sealed: false,
      source,
      live: liveText,
      english,
      state: live.length === run.length ? "open" : blockState(first.state),
      reason: first.reason,
      preview,
    });
  };

  for (const line of lines) {
    const seal = sealOf.get(line.id);
    if (seal !== undefined) {
      flushOpen();
      if (done.has(seal.id)) continue;
      done.add(seal.id);
      const leader = byId.get(seal.id);
      if (leader !== undefined) emit(seal.id, seal.ids, leader);
    } else if (isTerminal(line.state)) {
      flushOpen();
      emit(line.id, [line.id], line);
    } else {
      open.push(line);
    }
  }
  flushOpen();
  return blocks;
}

export interface FlowPiece {
  key: UtteranceId;
  text: string;
  /** "live" is Chinese still being spoken; "preview" is English that the next translation
   * replaces. */
  kind: "final" | "streaming" | "live" | "pending" | "preview" | "english";
}

export interface FlowPieces {
  source: FlowPiece[];
  english: FlowPiece[];
}

/** Running text as two paragraphs of the same blocks, so a block's Chinese and English always
 * cover the same words. */
export function flowPieces(blocks: readonly CaptionBlock[], showSource: boolean): FlowPieces {
  const source: FlowPiece[] = [];
  const english: FlowPiece[] = [];
  for (const block of blocks) {
    if (block.state === "english") {
      english.push({ key: block.key, text: block.source, kind: "english" });
      continue;
    }
    if (block.source !== "" && showSource) {
      source.push({ key: block.key, text: block.source, kind: "final" });
    }
    if (block.live !== "") source.push({ key: block.key, text: block.live, kind: "live" });
    if (block.preview !== null) english.push({ key: block.key, text: block.preview, kind: "preview" });
    else if (block.state === "final" || block.state === "streaming") {
      if (block.english !== "" || block.state === "streaming") {
        english.push({ key: block.key, text: block.english, kind: block.state });
      }
    } else if (block.state === "pending") {
      english.push({ key: block.key, text: "···", kind: "pending" });
    }
  }
  return { source, english };
}

interface AppSlice {
  modelsReady: boolean;
  listening: ListeningStateKind;
  source: SourceInfo | null;
  sourceState: SourceStateKind;
  sourceDetail: string | null;
  engines: Partial<Record<EngineKind, EngineState>>;
}

interface SettingsSlice {
  pauseHotkey: string;
  settingsHotkey: string;
  appNames: string[];
  liveSource: boolean;
}

export interface CaptionControllerOptions {
  clock?: Clock;
  onWarn?: (message: string) => void;
}

export type CaptionConfig = Pick<Config, "hotkeys" | "overlay" | "capture">;

export interface CaptionController extends Readable<CaptionView> {
  dispatch(event: PipelineEvent): void;
  setAppState(state: AppState | null): void;
  setConfig(config: CaptionConfig): void;
  dispose(): void;
}

export function createCaptionController(options: CaptionControllerOptions = {}): CaptionController {
  const clock = options.clock ?? systemClock;
  const warn = options.onWarn ?? (() => undefined);

  const lines = new Map<number, CaptionLine>();
  const seals = new Map<number, Seal>();
  let chipTimer: unknown = null;
  let chip: ChipMessage | null = null;
  let chipSeq = 0;
  let announcement = "";
  let lastStatusText: string | null = null;
  let disposed = false;
  let preview: Preview | null = null;
  let settings: SettingsSlice = {
    pauseHotkey: "",
    settingsHotkey: "",
    appNames: [],
    liveSource: true,
  };

  let app: AppSlice = {
    modelsReady: true,
    listening: "starting",
    source: null,
    sourceState: "playing",
    sourceDetail: null,
    engines: {},
  };

  const store = writable<CaptionView>({
    blocks: [],
    status: null,
    chip: null,
    announcement: "",
  });

  function status(): StatusLine | null {
    return deriveStatus({
      modelsReady: app.modelsReady,
      engines: app.engines,
      listening: app.listening,
      sourceState: app.sourceState,
      sourceDetail: app.sourceDetail,
      selectedAppNames: settings.appNames,
      pauseHotkey: settings.pauseHotkey,
      settingsHotkey: settings.settingsHotkey,
    });
  }

  function emit(): void {
    if (disposed) return;
    const current = status();
    const text = current?.text ?? null;
    if (text !== lastStatusText) {
      lastStatusText = text;
      if (text !== null) announcement = text;
    }
    store.set({
      blocks: captionBlocks([...lines.values()], seals, preview, settings.liveSource),
      status: current,
      chip: current === null ? chip : null,
      announcement,
    });
  }

  function clearTimer(handle: unknown): null {
    if (handle !== null) clock.clearTimer(handle);
    return null;
  }

  function showChip(text: string): void {
    chipTimer = clearTimer(chipTimer);
    chipSeq += 1;
    chip = { id: chipSeq, text };
    chipTimer = clock.setTimer(() => {
      chipTimer = null;
      chip = null;
      emit();
    }, CHIP_MS);
  }

  function prune(): void {
    const terminal = [...lines.values()].filter((line) => isTerminal(line.state));
    const dropped = new Set(terminal.slice(0, Math.max(0, terminal.length - MAX_HISTORY_LINES)).map((line) => line.id));
    const active = [...lines.values()].filter((line) => !isTerminal(line.state));
    for (const line of active.slice(0, Math.max(0, active.length - MAX_OUTSTANDING_LINES))) dropped.add(line.id);
    for (const id of dropped) lines.delete(id);
    for (const [id, seal] of seals) {
      if (seal.ids.every((covered) => dropped.has(covered))) seals.delete(id);
    }
  }

  function update(id: number, patch: Partial<CaptionLine>): void {
    const line = lines.get(id);
    if (line) lines.set(id, { ...line, ...patch, updatedAt: clock.now() });
  }

  function addLine(line: Omit<CaptionLine, "updatedAt">): void {
    lines.set(line.id, { ...line, updatedAt: clock.now() });
  }

  function newLine(id: number, source: string, state: CaptionLine["state"]): Omit<CaptionLine, "updatedAt"> {
    return {
      id,
      state,
      source,
      english: "",
      reason: null,
    };
  }

  function isActive(id: number): boolean {
    const line = lines.get(id);
    return line !== undefined && !isTerminal(line.state);
  }

  /** The final translation of any covered line replaces the preview. */
  function endPreview(id: number): void {
    if (preview?.ids.includes(id)) preview = null;
  }

  function applyLineEvent(event: PipelineEvent): boolean {
    switch (event.type) {
      case "translation_preview":
        if (!lines.has(event.id)) return false;
        preview = { id: event.id, ids: event.ids, text: event.text };
        return true;
      case "asr_partial": {
        const line = lines.get(event.id);
        if (line === undefined) {
          addLine(newLine(event.id, event.text, "live"));
          return true;
        }
        if (line.state !== "live") return false;
        update(event.id, { source: event.text });
        return true;
      }
      case "asr_final": {
        const existing = lines.get(event.id);
        if (existing !== undefined && existing.state !== "live") return false;
        const base = existing ?? newLine(event.id, event.text, "pending");
        let state: CaptionLine["state"] = "pending";
        if (event.class === "english") {
          state = "english";
          announcement = event.text;
        }
        // The English is translated from this text, so the Chinese shown is always this text too.
        lines.set(event.id, {
          ...base,
          state,
          source: event.text,
          updatedAt: clock.now(),
        });
        return true;
      }
      case "unit_sealed": {
        // Sealed lines get no more events of their own, so they go even when the leader was
        // already evicted.
        let changed = false;
        for (const absorbed of event.ids) {
          if (absorbed === event.id) continue;
          const line = lines.get(absorbed);
          if (line !== undefined && !isTerminal(line.state)) {
            lines.delete(absorbed);
            changed = true;
          }
        }
        const leader = lines.get(event.id);
        if (!leader) return changed;
        // A second seal of the same unit adds ids and keeps the first line as the key.
        seals.set(event.id, { id: event.id, ids: [...event.ids] });
        if (isTerminal(leader.state)) return true;
        update(event.id, { source: event.text });
        return true;
      }
      case "translation_delta": {
        const line = lines.get(event.id);
        if (!line || line.state === "live" || isTerminal(line.state)) return false;
        update(event.id, { state: "streaming", english: event.text_so_far });
        return true;
      }
      case "translation_final":
        if (!isActive(event.id)) return false;
        endPreview(event.id);
        update(event.id, { state: "final", english: event.text });
        announcement = event.text;
        return true;
      case "translation_failed":
        if (!isActive(event.id)) return false;
        endPreview(event.id);
        update(event.id, { state: "failed", reason: failReasonText(event.reason, event.message) });
        return true;
      case "skipped":
        if (!isActive(event.id)) return false;
        endPreview(event.id);
        update(event.id, { state: "skipped" });
        return true;
      case "dropped": {
        if (lines.get(event.id)?.state !== "live") return false;
        endPreview(event.id);
        lines.delete(event.id);
        return true;
      }
      default:
        return false;
    }
  }

  function dispatch(event: PipelineEvent): void {
    if (disposed) return;
    switch (event.type) {
      case "dropped":
        // A drop only concerns the store when a live line was already showing; otherwise it is not a line event.
        if (!applyLineEvent(event)) return;
        prune();
        emit();
        return;
      case "asr_partial":
      case "asr_final":
      case "unit_sealed":
      case "translation_delta":
      case "translation_final":
      case "translation_failed":
      case "skipped":
      case "translation_preview":
        if (!applyLineEvent(event)) {
          warn(`ignored ${event.type} for utterance ${event.id}`);
          return;
        }
        prune();
        emit();
        return;
      case "source_changed":
        app = { ...app, source: event.info };
        emit();
        return;
      case "source_state": {
        const changed = app.sourceState !== event.state;
        app = { ...app, sourceState: event.state, sourceDetail: event.detail };
        if (changed && event.state === "silent" && lines.size === 0) showChip(silentChipText(app.source));
        emit();
        return;
      }
      case "listening_state": {
        app = { ...app, listening: event.state, modelsReady: true };
        emit();
        return;
      }
      case "engine_status":
        app = { ...app, engines: { ...app.engines, [event.engine]: event.state } };
        emit();
        return;
      case "speech_started":
      case "stats":
        return;
    }
  }

  function setAppState(state: AppState | null): void {
    if (disposed || state === null) return;
    app = {
      modelsReady: state.models_ready,
      listening: state.listening,
      source: state.source,
      sourceState: state.source_state,
      sourceDetail: app.sourceDetail,
      engines: { ...state.engines },
    };
    emit();
  }

  function setConfig(config: CaptionConfig): void {
    if (disposed) return;
    settings = {
      pauseHotkey: config.hotkeys.pause,
      settingsHotkey: config.hotkeys.settings,
      appNames: config.capture.apps.map((entry) => entry.name),
      liveSource: config.overlay.live_source,
    };
    emit();
  }

  function dispose(): void {
    if (disposed) return;
    lines.clear();
    seals.clear();
    preview = null;
    chip = null;
    emit();
    disposed = true;
    chipTimer = clearTimer(chipTimer);
  }

  return { subscribe: store.subscribe, dispatch, setAppState, setConfig, dispose };
}
