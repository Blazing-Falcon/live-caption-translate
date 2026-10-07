import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { get, writable, type Readable } from "svelte/store";
import * as api from "./api";
import { createCaptionController, systemClock, type CaptionController, type Clock } from "./captions";
import type { Config, ConfigPatch, SetConfigResult } from "./settings";
import type {
  AppState,
  HotkeyError,
  ModelProgress,
  ModelStatus,
  OverlayModePayload,
  OverlayVisiblePayload,
  PipelineEvent,
  PipelineStats,
} from "./types";

export const EVENTS = {
  pipeline: "pipeline://event",
  overlayMode: "overlay://mode",
  overlayVisible: "overlay://visible",
  /** Optional, not in 07: native cursor-over-panel observation, `{ hover: boolean }`. */
  overlayHover: "overlay://hover",
  configChanged: "config://changed",
  modelsProgress: "models://progress",
  hotkeysError: "hotkeys://error",
} as const;

export type Disposer = () => void;

/**
 * Subscribes to a Tauri event. The returned disposer is safe to call before the
 * asynchronous registration resolves: the late unlisten function is invoked as
 * soon as it arrives and no payload is delivered after disposal.
 */
export function subscribe<T>(name: string, handler: (payload: T) => void): Disposer {
  let disposed = false;
  let unlisten: UnlistenFn | null = null;
  listen<T>(name, (event) => {
    if (!disposed) handler(event.payload);
  })
    .then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(() => undefined);
  return () => {
    disposed = true;
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
  };
}

/** Keeps the header/footer snapshot (get_state) current from the same event stream. */
export function applyEventToState(state: AppState, event: PipelineEvent): AppState {
  switch (event.type) {
    case "listening_state":
      return { ...state, listening: event.state, models_ready: true };
    case "source_changed":
      return { ...state, source: event.info };
    case "source_state":
      return { ...state, source_state: event.state };
    case "engine_status":
      return { ...state, engines: { ...state.engines, [event.engine]: event.state } };
    default:
      return state;
  }
}

export interface Notice {
  id: number;
  kind: "error" | "info" | "warning";
  text: string;
}

export type WindowKind = "overlay" | "control";

export interface SessionOptions {
  kind: WindowKind;
  clock?: Clock;
  reducedMotion?: () => boolean;
}

export interface Session {
  kind: WindowKind;
  captions: CaptionController;
  appState: Readable<AppState | null>;
  config: Readable<Config | null>;
  stats: Readable<PipelineStats | null>;
  models: Readable<ModelStatus[] | null>;
  hotkeyErrors: Readable<Record<string, HotkeyError>>;
  notices: Readable<Notice[]>;
  hover: Readable<boolean>;
  busy: Readable<ReadonlySet<string>>;
  loadError: Readable<string | null>;
  ready: Promise<void>;
  save(key: string, patch: ConfigPatch): Promise<SetConfigResult | null>;
  act<T>(key: string, action: () => Promise<T>): Promise<T | null>;
  refreshState(): Promise<void>;
  refreshModels(): Promise<ModelStatus[] | null>;
  setModels(models: ModelStatus[]): void;
  clearHotkeyError(action: string): void;
  pushNotice(kind: Notice["kind"], text: string): void;
  dismissNotice(id: number): void;
  dispose(): void;
}

const MAX_QUEUED_EVENTS = 500;
const INFO_NOTICE_MS = 8000;

export function appliedNotice(applied: SetConfigResult["applied"]): string | null {
  switch (applied) {
    case "restart":
      return "This change takes effect the next time the app starts.";
    case "pipeline":
      return "Applied. Speech recognition and translation restart briefly.";
    default:
      return null;
  }
}

export function createSession(options: SessionOptions): Session {
  const clock = options.clock ?? systemClock;
  const captions = createCaptionController({ clock, reducedMotion: options.reducedMotion });
  const appState = writable<AppState | null>(null);
  const config = writable<Config | null>(null);
  const stats = writable<PipelineStats | null>(null);
  const models = writable<ModelStatus[] | null>(null);
  const hotkeyErrors = writable<Record<string, HotkeyError>>({});
  const notices = writable<Notice[]>([]);
  const hover = writable(false);
  const busy = writable<ReadonlySet<string>>(new Set());
  const loadError = writable<string | null>(null);

  const disposers: Disposer[] = [];
  const noticeTimers = new Map<number, unknown>();
  let noticeSeq = 0;
  let loaded = false;
  let disposed = false;
  const queued: PipelineEvent[] = [];

  function handlePipeline(event: PipelineEvent): void {
    if (event.type === "stats") {
      const { type: _type, ...rest } = event;
      stats.set(rest);
      return;
    }
    appState.update((state) => (state ? applyEventToState(state, event) : state));
    captions.dispatch(event);
  }

  function applyConfig(next: Config): void {
    config.set(next);
    captions.setConfig(next);
  }

  function dismissNotice(id: number): void {
    const timer = noticeTimers.get(id);
    if (timer !== undefined) clock.clearTimer(timer);
    noticeTimers.delete(id);
    notices.update((list) => list.filter((notice) => notice.id !== id));
  }

  function pushNotice(kind: Notice["kind"], text: string): void {
    if (disposed || get(notices).some((notice) => notice.text === text && notice.kind === kind)) return;
    noticeSeq += 1;
    const id = noticeSeq;
    notices.update((list) => [...list, { id, kind, text }]);
    if (kind === "info") noticeTimers.set(id, clock.setTimer(() => dismissNotice(id), INFO_NOTICE_MS));
  }

  function setBusy(key: string, on: boolean): void {
    busy.update((current) => {
      const next = new Set(current);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });
  }

  async function act<T>(key: string, action: () => Promise<T>): Promise<T | null> {
    setBusy(key, true);
    try {
      return await action();
    } catch (error) {
      pushNotice("error", api.readableError(error));
      return null;
    } finally {
      setBusy(key, false);
    }
  }

  async function save(key: string, patch: ConfigPatch): Promise<SetConfigResult | null> {
    const result = await act(key, () => api.setConfig(patch));
    if (!result) return null;
    applyConfig(result.config);
    for (const message of result.messages) pushNotice("info", message);
    const note = appliedNotice(result.applied);
    if (note) pushNotice("info", note);
    return result;
  }

  async function refreshState(): Promise<void> {
    try {
      const state = await api.getState();
      if (disposed) return;
      appState.set(state);
      captions.setAppState(state);
    } catch (error) {
      loadError.set(api.readableError(error));
    }
  }

  async function refreshModels(): Promise<ModelStatus[] | null> {
    try {
      const rows = await api.modelsStatus();
      if (!disposed) models.set(rows);
      return rows;
    } catch (error) {
      pushNotice("error", api.readableError(error));
      return null;
    }
  }

  disposers.push(
    subscribe<PipelineEvent>(EVENTS.pipeline, (event) => {
      if (!loaded) {
        if (queued.length < MAX_QUEUED_EVENTS) queued.push(event);
        return;
      }
      handlePipeline(event);
    }),
    subscribe<Config>(EVENTS.configChanged, (next) => applyConfig(next)),
    subscribe<OverlayModePayload>(EVENTS.overlayMode, ({ moving }) =>
      appState.update((state) => (state ? { ...state, overlay_moving: moving } : state)),
    ),
    subscribe<OverlayVisiblePayload>(EVENTS.overlayVisible, ({ visible }) =>
      appState.update((state) => (state ? { ...state, overlay_visible: visible } : state)),
    ),
    subscribe<{ hover: boolean }>(EVENTS.overlayHover, (payload) => hover.set(payload.hover)),
  );

  if (options.kind === "control") {
    disposers.push(
      subscribe<ModelProgress[]>(EVENTS.modelsProgress, (rows) => models.set(rows)),
      subscribe<HotkeyError>(EVENTS.hotkeysError, (error) =>
        hotkeyErrors.update((current) => ({ ...current, [error.action]: error })),
      ),
    );
  }

  const ready = (async () => {
    const [stateResult, configResult] = await Promise.allSettled([api.getState(), api.getConfig()]);
    if (disposed) return;
    if (configResult.status === "fulfilled") applyConfig(configResult.value);
    else loadError.set(api.readableError(configResult.reason));
    if (stateResult.status === "fulfilled") {
      appState.set(stateResult.value);
      captions.setAppState(stateResult.value);
    } else {
      loadError.set(api.readableError(stateResult.reason));
    }
    loaded = true;
    for (const event of queued.splice(0)) handlePipeline(event);
    if (options.kind === "control") {
      await refreshModels();
      api.getStats().then((value) => !disposed && stats.set(value)).catch(() => undefined);
      api
        .startupNotices()
        .then((list) => list.forEach((text) => pushNotice("warning", text)))
        .catch(() => undefined);
    }
  })();

  function dispose(): void {
    if (disposed) return;
    disposed = true;
    for (const dispose of disposers.splice(0)) dispose();
    for (const timer of noticeTimers.values()) clock.clearTimer(timer);
    noticeTimers.clear();
    captions.dispose();
  }

  return {
    kind: options.kind,
    captions,
    appState,
    config,
    stats,
    models,
    hotkeyErrors,
    notices,
    hover,
    busy,
    loadError,
    ready,
    save,
    act,
    refreshState,
    refreshModels,
    setModels: (rows) => models.set(rows),
    clearHotkeyError: (action) =>
      hotkeyErrors.update((current) => {
        const { [action]: _removed, ...rest } = current;
        return rest;
      }),
    pushNotice,
    dismissNotice,
    dispose,
  };
}
