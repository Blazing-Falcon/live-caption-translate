/** Mirrors lt-core::config; Rust validates patches before persisting them. */
export interface CaptureApp {
  exe: string;
  name: string;
}

export interface OverlayRect {
  monitor: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Config {
  screen: { direction: "horizontal" | "vertical" };
  config_version: number;
  app: { theme: "system" | "light" | "dark" };
  capture: {
    mode: "system" | "apps";
    device: string;
    apps: CaptureApp[];
    autostart: boolean;
  };
  audio: {
    normalize: boolean;
  };
  asr: {
    threads: number;
    pause_s: number;
    second_pass: boolean;
    second_pass_threads: number;
  };
  translate: {
    engine: "local" | "microsoft" | "google" | "google_beta" | "bing" | "youdao" | "openai";
    fallback: boolean;
    openai: { base_url: string; model: string; api_key: string; prompt: string };
    threads: number;
  };
  overlay: {
    visible: boolean;
    style: "panel" | "flow";
    font_px: number;
    background: number;
    show_source: boolean;
    panel_edge: "left" | "right";
    panel_lines: number;
    flow_lines: number;
    live_source: boolean;
    card: "night" | "outline";
    font: "rounded" | "clear" | "system";
    /** `#rrggbb` */
    english_color: string;
    /** `#rrggbb` */
    chinese_color: string;
    panel_rect?: OverlayRect;
    flow_rect?: OverlayRect;
  };
  performance: {
    low_priority: boolean;
  };
  hotkeys: {
    move_lock: string;
    show_hide: string;
    pause: string;
    settings: string;
    screen_translate: string;
  };
  transcript: {
    enabled: boolean;
    retention_days: number;
  };
  logging: {
    level: "error" | "warn" | "info" | "debug" | "trace";
    keep_files: number;
  };
  models: {
    dir: string;
  };
}

export type DeepPartial<T> = T extends (infer Item)[]
  ? DeepPartial<Item>[]
  : T extends object
    ? { [Key in keyof T]?: DeepPartial<T[Key]> | null }
    : T;

export type AppliedKind = "live" | "capture" | "pipeline" | "restart";

export interface SetConfigResult {
  config: Config;
  applied: AppliedKind;
  messages: string[];
}

export type ConfigPatch = DeepPartial<Config>;

export const OVERLAY_LIMITS = {
  font_px: { min: 12, max: 40 },
  background: { min: 0, max: 1 },
  panel_lines: { min: 1, max: 10 },
  flow_lines: { min: 1, max: 10 },
} as const;

/** The most threads a setting accepts; the UI offers up to the PC's logical CPUs. */
export const MAX_THREADS = 64;
/** Below this the English cannot reach 4.5:1 against white video, so the panel warns. The
 * value itself fails the floor, hence "at or below". */
export const LOW_BACKGROUND_WARNING = 0.5;

export const HOTKEY_ACTIONS = ["move_lock", "show_hide", "pause", "settings", "screen_translate"] as const;
export type HotkeyAction = (typeof HOTKEY_ACTIONS)[number];

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

const CONFIG_SECTIONS = [
  "app", "capture", "audio", "asr", "translate",
  "overlay", "screen", "performance", "hotkeys", "transcript", "logging", "models",
] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Rejects payloads that are not a full config before the UI renders from them. */
export function parseConfig(value: unknown): Config {
  if (!isRecord(value) || typeof value.config_version !== "number") {
    throw new Error("The app returned an unreadable settings file.");
  }
  for (const table of CONFIG_SECTIONS) {
    if (!isRecord(value[table])) {
      throw new Error(`The settings are missing the [${table}] section.`);
    }
  }
  return value as unknown as Config;
}

/** Deep merge with the patch semantics the UI assumes: arrays replace, null removes a key. */
export function applyPatch<T extends object>(base: T, patch: DeepPartial<T>): T {
  const out: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [key, value] of Object.entries(patch as Record<string, unknown>)) {
    if (value === undefined) continue;
    if (value === null) {
      delete out[key];
    } else if (isRecord(value) && isRecord(out[key])) {
      out[key] = applyPatch(out[key] as object, value as DeepPartial<object>);
    } else {
      out[key] = structuredClone(value);
    }
  }
  return out as T;
}

export function sameExe(a: string, b: string): boolean {
  return a.toLowerCase() === b.toLowerCase();
}

export function hasApp(apps: readonly CaptureApp[], exe: string): boolean {
  return apps.some((app) => sameExe(app.exe, exe));
}

/** Returns the full replacement array for capture.apps; entries are matched by exe, case-insensitively. */
export function toggledApps(apps: readonly CaptureApp[], entry: CaptureApp, selected: boolean): CaptureApp[] {
  const rest = apps.filter((app) => !sameExe(app.exe, entry.exe));
  return selected ? [...rest, { exe: entry.exe, name: entry.name }] : rest;
}

export function captureModePatch(mode: Config["capture"]["mode"]): ConfigPatch {
  return { capture: { mode } };
}

export function captureDevicePatch(device: string): ConfigPatch {
  return { capture: { device } };
}

export function captureAppsPatch(apps: CaptureApp[]): ConfigPatch {
  return { capture: { apps } };
}

export function overlayPatch(values: DeepPartial<Config["overlay"]>): ConfigPatch {
  return { overlay: values };
}

export function hotkeyPatch(action: HotkeyAction, accelerator: string): ConfigPatch {
  return { hotkeys: { [action]: accelerator.trim() } };
}

export function threadsPatch(table: "asr" | "translate", threads: number): ConfigPatch {
  return { [table]: { threads: clamp(Math.round(threads), 1, MAX_THREADS) } };
}

export function transcriptPatch(enabled: boolean): ConfigPatch {
  return { transcript: { enabled } };
}

export function performancePatch(values: DeepPartial<Config["performance"]>): ConfigPatch {
  return { performance: values };
}

export function asrPatch(values: DeepPartial<Config["asr"]>): ConfigPatch {
  return { asr: values };
}

/** "Ctrl+Shift+P" -> "Ctrl Shift P" as the spec prints bindings. */
export function formatAccelerator(accelerator: string): string {
  return accelerator.split("+").map((part) => part.trim()).filter(Boolean).join(" ");
}

/** Manifest ids the control window names. */
export const SECOND_PASS_MODEL_ID = "xasr-offline-int8-2026-06-03";
export const TRANSLATOR_MODEL_ID = "hy-mt2-1.8b-ternary-tq2_0";
export const SPEECH_MODEL_ID = "xasr-stream-480ms-int8-2026-06-05";

/** The key part of a Tauri accelerator for a pressed key, or null for keys a shortcut cannot use. */
function acceleratorKey(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  const named: Record<string, string> = {
    Space: "Space",
    Insert: "Insert",
    Delete: "Delete",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
  };
  return named[code] ?? null;
}

export type RecordedKeys = { kind: "shortcut"; accelerator: string } | { kind: "incomplete" } | { kind: "cancel" } | { kind: "clear" };

/** Reads a key press while a shortcut is being recorded. A shortcut needs a modifier unless it is a function key. */
export function recordShortcut(event: Pick<KeyboardEvent, "code" | "key" | "ctrlKey" | "altKey" | "shiftKey" | "metaKey">): RecordedKeys {
  const modifiers = [event.ctrlKey && "Ctrl", event.altKey && "Alt", event.shiftKey && "Shift", event.metaKey && "Super"].filter(Boolean) as string[];
  if (modifiers.length === 0 && event.key === "Escape") return { kind: "cancel" };
  if (modifiers.length === 0 && (event.key === "Backspace" || event.key === "Delete")) return { kind: "clear" };
  const key = acceleratorKey(event.code);
  if (key === null) return { kind: "incomplete" };
  if (modifiers.length === 0 && !/^F\d+$/.test(key)) return { kind: "incomplete" };
  return { kind: "shortcut", accelerator: [...modifiers, key].join("+") };
}

/** Kept as a JSON initializer so the Rust parity test can read it directly. */
export const DEFAULT_CONFIG: Config = {
  "screen": { "direction": "horizontal" },
  "config_version": 4,
  "app": { "theme": "system" },
  "capture": {
    "mode": "system",
    "device": "default",
    "apps": [],
    "autostart": true
  },
  "audio": {
    "normalize": true
  },
  "asr": {
    "threads": 1,
    "pause_s": 0.8,
    "second_pass": true,
    "second_pass_threads": 2
  },
  "translate": {
    "fallback": true,
    "engine": "local",
    "openai": { "base_url": "", "model": "", "api_key": "", "prompt": "" },
    "threads": 4
  },
  "overlay": {
    "visible": true,
    "style": "panel",
    "font_px": 26,
    "background": 0.82,
    "show_source": true,
    "panel_edge": "right",
    "panel_lines": 5,
    "flow_lines": 3,
    "live_source": true,
    "card": "night",
    "font": "rounded",
    "english_color": "#ffffff",
    "chinese_color": "#c9c9c4"
  },
  "performance": {
    "low_priority": true
  },
  "hotkeys": {
    "move_lock": "Ctrl+Shift+L",
    "show_hide": "Ctrl+Shift+H",
    "settings": "Ctrl+Shift+S",
    "pause": "Ctrl+Shift+P",
    "screen_translate": "Ctrl+Shift+A"
  },
  "transcript": {
    "enabled": true,
    "retention_days": 30
  },
  "logging": {
    "level": "info",
    "keep_files": 5
  },
  "models": {
    "dir": ""
  }
};
