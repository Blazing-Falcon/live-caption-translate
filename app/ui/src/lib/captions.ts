import { readable, writable, type Readable } from "svelte/store";
import { formatAccelerator, isOtherLangTranslated, normalizeLang, type Config } from "./settings";
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
} from "./types";

export const FADE_MS = 200;
export const CHIP_MS = 3000;
/** Terminal rows kept after they leave the screen; the panel shows at most 6. */
export const MAX_HISTORY_LINES = 6;
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

export interface CaptionView {
  lines: readonly CaptionLine[];
  fadingIds: ReadonlySet<number>;
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
}

const ENGINE_ORDER: readonly EngineKind[] = ["vad", "asr", "translator"];

export function isTerminal(state: CaptionLine["state"]): boolean {
  return state !== "pending" && state !== "streaming";
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

export function pausedText(pauseHotkey: string): string {
  const binding = formatAccelerator(pauseHotkey);
  return binding === "" ? "Paused · Resume in Settings" : `Paused · ${binding} to resume`;
}

/** Priority order is fixed; silence is handled by the chip. */
export function deriveStatus(input: StatusInput): StatusLine | null {
  if (!input.modelsReady) {
    return { kind: "models_missing", text: "Models not downloaded. Open settings to download." };
  }
  for (const engine of ENGINE_ORDER) {
    if (input.engines[engine] === "failed") {
      const name = engine === "translator" ? "Translator" : "Speech recognition";
      return { kind: "engine_failed", text: `${name} stopped working. Open settings for details.` };
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

export type OverlayStyle = "bar" | "panel";

/** Newest line is full strength; older lines fade by index and stop at the last step. */
const AGE_OPACITY: Record<OverlayStyle, readonly number[]> = {
  bar: [1, 0.55],
  panel: [1, 0.75, 0.6, 0.45],
};

export function ageOpacity(style: OverlayStyle, age: number): number {
  const steps = AGE_OPACITY[style];
  return steps[Math.min(Math.max(age, 0), steps.length - 1)] ?? 1;
}

export function visibleLines(lines: readonly CaptionLine[], style: OverlayStyle, panelLines: number): CaptionLine[] {
  const count = style === "bar" ? 2 : Math.min(6, Math.max(4, Math.round(panelLines)));
  return lines.slice(-count);
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
  translateOther: string[];
  pauseHotkey: string;
  expireMs: number;
  appNames: string[];
}

export interface CaptionControllerOptions {
  clock?: Clock;
  reducedMotion?: () => boolean;
  onWarn?: (message: string) => void;
}

export type CaptionConfig = Pick<Config, "routing" | "hotkeys" | "overlay" | "capture">;

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

  const lines = new Map<number, CaptionLine>();
  const fading = new Set<number>();
  let lastAsrFinalAt: number | null = null;
  let expiryTimer: unknown = null;
  let fadeTimer: unknown = null;
  let chipTimer: unknown = null;
  let chip: ChipMessage | null = null;
  let chipSeq = 0;
  let announcement = "";
  let lastStatusText: string | null = null;
  let disposed = false;

  let app: AppSlice = {
    modelsReady: true,
    listening: "starting",
    source: null,
    sourceState: "playing",
    sourceDetail: null,
    engines: {},
  };
  let settings: SettingsSlice = { translateOther: [], pauseHotkey: "", expireMs: 8000, appNames: [] };

  const store = writable<CaptionView>({
    lines: [],
    fadingIds: new Set(),
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
      lines: [...lines.values()],
      fadingIds: new Set(fading),
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

  function expirable(): number[] {
    return [...lines.values()].filter((line) => isTerminal(line.state) && !fading.has(line.id)).map((line) => line.id);
  }

  function armExpiry(): void {
    expiryTimer = clearTimer(expiryTimer);
    if (lastAsrFinalAt === null || expirable().length === 0) return;
    const delay = Math.max(0, lastAsrFinalAt + settings.expireMs - clock.now());
    expiryTimer = clock.setTimer(onExpire, delay);
  }

  function onExpire(): void {
    expiryTimer = null;
    const ids = expirable();
    if (ids.length === 0) return;
    if (reducedMotion()) {
      for (const id of ids) lines.delete(id);
      emit();
      return;
    }
    for (const id of ids) fading.add(id);
    fadeTimer = clearTimer(fadeTimer);
    fadeTimer = clock.setTimer(() => {
      fadeTimer = null;
      for (const id of fading) lines.delete(id);
      fading.clear();
      emit();
      armExpiry();
    }, FADE_MS);
    emit();
  }

  function cancelFade(): void {
    fadeTimer = clearTimer(fadeTimer);
    fading.clear();
  }

  function prune(): void {
    const terminal = [...lines.values()].filter((line) => isTerminal(line.state));
    for (const line of terminal.slice(0, Math.max(0, terminal.length - MAX_HISTORY_LINES))) {
      lines.delete(line.id);
      fading.delete(line.id);
    }
    const active = [...lines.values()].filter((line) => !isTerminal(line.state));
    for (const line of active.slice(0, Math.max(0, active.length - MAX_OUTSTANDING_LINES))) {
      lines.delete(line.id);
    }
  }

  function update(id: number, patch: Partial<CaptionLine>): void {
    const line = lines.get(id);
    if (line) lines.set(id, { ...line, ...patch, updatedAt: clock.now() });
  }

  function addLine(line: Omit<CaptionLine, "updatedAt">): void {
    lines.set(line.id, { ...line, updatedAt: clock.now() });
  }

  function isActive(id: number): boolean {
    const line = lines.get(id);
    return line !== undefined && !isTerminal(line.state);
  }

  function applyLineEvent(event: PipelineEvent): boolean {
    switch (event.type) {
      case "asr_final": {
        if (lines.has(event.id)) return false;
        cancelFade();
        lastAsrFinalAt = clock.now();
        const lang = normalizeLang(event.lang);
        const base = { id: event.id, source: event.text, english: "", lang, reason: null };
        if (event.class === "english") {
          addLine({ ...base, state: "english" });
          announcement = event.text;
        } else if (event.class === "other") {
          const translated = isOtherLangTranslated(settings.translateOther, lang);
          addLine({ ...base, state: translated ? "pending" : "other" });
          if (!translated) announcement = event.text;
        } else {
          addLine({ ...base, state: "pending" });
        }
        return true;
      }
      case "joined": {
        const leader = lines.get(event.id);
        if (!leader || isTerminal(leader.state)) return false;
        for (const absorbed of event.absorbed) {
          if (absorbed !== event.id && lines.get(absorbed)?.state === "pending") lines.delete(absorbed);
        }
        update(event.id, { source: event.text });
        return true;
      }
      case "translation_delta":
        if (!isActive(event.id)) return false;
        update(event.id, { state: "streaming", english: event.text_so_far });
        return true;
      case "translation_final":
        if (!isActive(event.id)) return false;
        update(event.id, { state: "final", english: event.text });
        announcement = event.text;
        return true;
      case "translation_failed":
        if (!isActive(event.id)) return false;
        update(event.id, { state: "failed", reason: failReasonText(event.reason, event.message) });
        return true;
      case "skipped":
        if (!isActive(event.id)) return false;
        update(event.id, { state: "skipped" });
        return true;
      default:
        return false;
    }
  }

  function dispatch(event: PipelineEvent): void {
    if (disposed) return;
    switch (event.type) {
      case "asr_final":
      case "joined":
      case "translation_delta":
      case "translation_final":
      case "translation_failed":
      case "skipped":
        if (!applyLineEvent(event)) {
          warn(`ignored ${event.type} for utterance ${event.id}`);
          return;
        }
        prune();
        armExpiry();
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
      case "asr_partial":
      case "dropped":
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
      translateOther: [...config.routing.translate_other],
      pauseHotkey: config.hotkeys.pause,
      expireMs: Math.max(0, config.overlay.expire_s * 1000),
      appNames: config.capture.apps.map((entry) => entry.name),
    };
    armExpiry();
    emit();
  }

  function dispose(): void {
    if (disposed) return;
    lines.clear();
    fading.clear();
    chip = null;
    emit();
    disposed = true;
    expiryTimer = clearTimer(expiryTimer);
    fadeTimer = clearTimer(fadeTimer);
    chipTimer = clearTimer(chipTimer);
  }

  return { subscribe: store.subscribe, dispatch, setAppState, setConfig, dispose };
}

/** Fixed view for rendering fixtures and component tests. */
export function staticView(partial: Partial<CaptionView>): Readable<CaptionView> {
  return readable({ lines: [], fadingIds: new Set<number>(), status: null, chip: null, announcement: "", ...partial });
}
