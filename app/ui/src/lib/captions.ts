import { writable, type Readable } from "svelte/store";
import { OVERLAY_LIMITS, formatAccelerator, type Config } from "./settings";
import type {
  AppState,
  CaptionLine,
  EngineKind,
  EngineState,
  FailReason,
  ListeningStateKind,
  PipelineEvent,
  SecondPass,
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

export function prefersReducedMotion(): boolean {
  try {
    return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

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

export interface CaptionView {
  lines: readonly CaptionLineView[];
  preview: Preview | null;
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

export function listeningChipText(source: SourceInfo | null): string {
  const label = source?.label.trim();
  return label ? `Listening · ${label}` : "Listening";
}

/** Newest line is full strength; older lines fade by index and stop at the last step. The
 * dimmest step stays at 0.6: below that the English drops under 4.5:1 against white video. */
const AGE_OPACITY: readonly number[] = [1, 0.75, 0.6];

export function ageOpacity(age: number): number {
  return AGE_OPACITY[Math.min(Math.max(age, 0), AGE_OPACITY.length - 1)] ?? 1;
}

/** Whether the second pass changed the live text by at most one character. Its distance counts
 * letters and digits only, so a punctuation-only correction counts as none. */
function isTrivialSwap(secondPass: SecondPass): boolean {
  const distance = secondPass.distance;
  return secondPass.live_text !== undefined && distance !== undefined && distance <= 1;
}

/** How long the display takes to fade one Chinese row out and the corrected one in. */
export const SWAP_MS = 150;

/** The Chinese a line showed before the second pass replaced it. The display fades it out
 * under the corrected text, so a reader can see that the line changed under them. */
export interface CaptionSwap {
  from: string;
  /** Counts replacements, so a row that re-enters the view does not replay the crossfade. */
  seq: number;
}

/** A caption line with what the display needs beyond the text itself. */
export interface CaptionLineView extends CaptionLine {
  swap?: CaptionSwap;
  /** English that the next translation replaces: the preview of the lines this row stands for. */
  preview?: string;
}

/** The panel's rows with the lines a preview covers shown as one row, in the place of the first:
 * its Chinese is theirs joined and its English is the preview, as the row will look once they are
 * translated together. Lines already translated for good keep their own rows. */
export function withPreview(
  lines: readonly CaptionLineView[],
  preview: Preview | null,
  liveSource: boolean,
): CaptionLineView[] {
  if (preview === null) return [...lines];
  const covered = (line: CaptionLineView): boolean =>
    preview.ids.includes(line.id) && (line.state === "live" || line.state === "pending" || line.state === "streaming");
  const parts = lines.filter(covered);
  const first = parts[0];
  if (first === undefined) return [...lines];
  const source = parts
    .filter((line) => line.state !== "live" || liveSource)
    .map((line) => line.source)
    .join("");
  const row: CaptionLineView = { ...first, state: "pending", source, english: "", preview: preview.text };
  delete row.swap;
  return lines.filter((line) => line === first || !covered(line)).map((line) => (line === first ? row : line));
}

/** A live line with its Chinese hidden has nothing to show, so it takes no place and no space. */
export function isHiddenLive(line: CaptionLine, liveSource: boolean): boolean {
  return line.state === "live" && !liveSource;
}

export function visibleLines<T extends CaptionLine>(
  lines: readonly T[],
  wanted: number,
  liveSource = true,
): T[] {
  const count = Math.min(OVERLAY_LIMITS.panel_lines.max, Math.max(OVERLAY_LIMITS.panel_lines.min, Math.round(wanted)));
  return lines.filter((line) => !isHiddenLive(line, liveSource)).slice(-count);
}

export interface FlowPiece {
  id: UtteranceId;
  text: string;
  /** "live" is Chinese still being spoken; "preview" is English that the next translation
   * replaces. */
  kind: "final" | "streaming" | "live" | "pending" | "preview";
}

export interface FlowText {
  source: FlowPiece[];
  english: FlowPiece[];
}

/** The lines as two running paragraphs, oldest first. Every line keeps its place, so a
 * line's English appears where its Chinese was and replaces its pending dots in place. The
 * preview stands for the English of every line it covers until their final translation, so
 * the text does not jump back to dots or restart while the final streams. */
export function flowText(
  lines: readonly CaptionLine[],
  showSource: boolean,
  liveSource: boolean,
  preview: Preview | null = null,
): FlowText {
  const source: FlowPiece[] = [];
  const english: FlowPiece[] = [];
  let previewShown = false;
  for (const line of lines) {
    const { id } = line;
    if (line.state === "english") {
      english.push({ id, text: line.source, kind: "final" });
      continue;
    }
    if (line.state === "live") {
      if (liveSource && line.source !== "") source.push({ id, text: line.source, kind: "live" });
    } else if (showSource && line.source !== "") {
      source.push({ id, text: line.source, kind: "final" });
    }
    const covered = preview !== null && preview.ids.includes(id);
    if (covered && (line.state === "live" || line.state === "pending" || line.state === "streaming")) {
      if (!previewShown) english.push({ id: preview.id, text: preview.text, kind: "preview" });
      previewShown = true;
      continue;
    }
    if (line.state === "final" || line.state === "streaming") {
      if (line.english !== "" || line.state === "streaming") english.push({ id, text: line.english, kind: line.state });
    } else if (line.state === "pending") {
      english.push({ id, text: "···", kind: "pending" });
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
}

export interface CaptionControllerOptions {
  clock?: Clock;
  reducedMotion?: () => boolean;
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
  const reducedMotion = options.reducedMotion ?? prefersReducedMotion;
  const warn = options.onWarn ?? (() => undefined);

  const lines = new Map<number, CaptionLineView>();
  /** The text a line shows while its replacement fades in; empty again after `SWAP_MS`. */
  const swaps = new Map<number, CaptionSwap>();
  const swapTimers = new Map<number, unknown>();
  let swapSeq = 0;
  let chipTimer: unknown = null;
  let chip: ChipMessage | null = null;
  let chipSeq = 0;
  let announcement = "";
  let lastStatusText: string | null = null;
  let disposed = false;
  let preview: Preview | null = null;

  let app: AppSlice = {
    modelsReady: true,
    listening: "starting",
    source: null,
    sourceState: "playing",
    sourceDetail: null,
    engines: {},
  };
  let settings: SettingsSlice = {
    pauseHotkey: "",
    settingsHotkey: "",
    appNames: [],
  };

  const store = writable<CaptionView>({
    lines: [],
    preview: null,
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
      lines: [...lines.values()].map((line) => {
        const swap = swaps.get(line.id);
        return swap === undefined ? line : { ...line, swap };
      }),
      preview,
      status: current,
      chip: current === null ? chip : null,
      announcement,
    });
  }

  /** Marks a line as swapping, so the display can fade the old Chinese out under the new one.
   * A second swap before the first ends replaces it; the view drops the old text when the
   * timer fires. */
  function startSwap(id: number, from: string): void {
    if (reducedMotion()) return;
    swapSeq += 1;
    swaps.set(id, { from, seq: swapSeq });
    const running = swapTimers.get(id);
    if (running !== undefined) clock.clearTimer(running);
    swapTimers.set(
      id,
      clock.setTimer(() => {
        swapTimers.delete(id);
        swaps.delete(id);
        emit();
      }, SWAP_MS),
    );
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
    for (const line of terminal.slice(0, Math.max(0, terminal.length - MAX_HISTORY_LINES))) {
      lines.delete(line.id);
    }
    const active = [...lines.values()].filter((line) => !isTerminal(line.state));
    for (const line of active.slice(0, Math.max(0, active.length - MAX_OUTSTANDING_LINES))) {
      lines.delete(line.id);
    }
  }

  function update(id: number, patch: Partial<CaptionLineView>): void {
    const line = lines.get(id);
    if (line) lines.set(id, { ...line, ...patch, updatedAt: clock.now() });
  }

  function addLine(line: Omit<CaptionLineView, "updatedAt">): void {
    lines.set(line.id, { ...line, updatedAt: clock.now() });
  }

  function newLine(id: number, source: string, state: CaptionLine["state"]): Omit<CaptionLineView, "updatedAt"> {
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
        // The English is translated from this text, so the Chinese shown is always this text too,
        // even when the correction is one character: that is the usual size of a misheard word.
        lines.set(event.id, {
          ...base,
          state,
          source: event.text,
          updatedAt: clock.now(),
        });
        // A larger correction changes what the viewer is reading, so the old Chinese fades out
        // under the new one. A one-character or punctuation fix is replaced in place: a crossfade
        // of the whole row would draw more attention than the change itself.
        const small = isTrivialSwap(event.second_pass);
        if (!small && existing !== undefined && existing.source !== "" && existing.source !== event.text) {
          startSwap(event.id, existing.source);
        }
        return true;
      }
      case "joined": {
        // Absorbed lines get no more events, so they go even when the leader was already evicted.
        let changed = false;
        for (const absorbed of event.absorbed) {
          const line = lines.get(absorbed);
          if (absorbed !== event.id && line !== undefined && !isTerminal(line.state)) {
            lines.delete(absorbed);
            changed = true;
          }
        }
        const leader = lines.get(event.id);
        if (!leader || isTerminal(leader.state)) return changed;
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
      case "joined":
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
        const changed = app.listening !== event.state;
        app = { ...app, listening: event.state, modelsReady: true };
        if (changed && event.state === "listening") showChip(listeningChipText(app.source));
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
    };
    emit();
  }

  function dispose(): void {
    if (disposed) return;
    lines.clear();
    preview = null;
    chip = null;
    emit();
    disposed = true;
    chipTimer = clearTimer(chipTimer);
  }

  return { subscribe: store.subscribe, dispatch, setAppState, setConfig, dispose };
}
