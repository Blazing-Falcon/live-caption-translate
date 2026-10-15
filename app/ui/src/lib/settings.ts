/** Mirrors lt-core::config; Rust validates patches before persisting them. */
interface ExtraFields {
  [key: string]: unknown;
}

export interface CaptureApp extends ExtraFields {
  exe: string;
  name: string;
}

export interface OverlayRect extends ExtraFields {
  monitor: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Config extends ExtraFields {
  config_version: number;
  capture: ExtraFields & {
    mode: "system" | "apps";
    device: string;
    apps: CaptureApp[];
    autostart: boolean;
  };
  audio: ExtraFields & {
    normalize: boolean;
    norm_attack_s: number;
    norm_release_s: number;
    norm_max_gain_db: number;
    norm_gate_dbfs: number;
  };
  asr: ExtraFields & {
    chunk_ms: 160 | 480 | 960;
    threads: number;
    pause_s: number;
    second_pass: "off" | "sensevoice" | "qwen3";
    second_pass_threads: number;
    second_pass_max_wait_s: number;
  };
  filter: ExtraFields & {
    single_char_max_s: number;
    single_char_allow: string[];
    fillers: string[];
  };
  translate: ExtraFields & {
    server_url: string;
    model: "ternary" | "q4_0";
    threads: number;
    ctx: number;
    temperature: number;
    repeat_penalty: number;
    max_tokens_cap: number;
    timeout_s: number;
    queue_join_max_chars: number;
    skip_lag_s: number;
    window_chars: number;
  };
  overlay: ExtraFields & {
    visible: boolean;
    style: "bar" | "panel" | "flow";
    font_px: number;
    background: number;
    show_source: boolean;
    expire_s: number;
    panel_edge: "left" | "right";
    panel_lines: number;
    flow_lines: number;
    live_source: boolean;
    bar_rect?: OverlayRect;
    panel_rect?: OverlayRect;
    flow_rect?: OverlayRect;
  };
  performance: ExtraFields & {
    low_priority: boolean;
  };
  hotkeys: ExtraFields & {
    move_lock: string;
    show_hide: string;
    pause: string;
  };
  transcript: ExtraFields & {
    enabled: boolean;
    retention_days: number;
  };
  logging: ExtraFields & {
    level: "error" | "warn" | "info" | "debug" | "trace";
    keep_files: number;
  };
  models: ExtraFields & {
    source: "huggingface" | "modelscope";
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
  font_px: { min: 18, max: 40 },
  background: { min: 0, max: 1 },
  expire_s: { min: 3, max: 60 },
  panel_lines: { min: 4, max: 6 },
  flow_lines: { min: 2, max: 6 },
} as const;

/** The most threads a setting accepts; the UI offers up to the PC's logical CPUs. */
export const MAX_THREADS = 64;
export const LOW_BACKGROUND_WARNING = 0.5;

export const HOTKEY_ACTIONS = ["move_lock", "show_hide", "pause"] as const;
export type HotkeyAction = (typeof HOTKEY_ACTIONS)[number];

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

const CONFIG_TABLES = [
  "capture", "audio", "asr", "filter", "translate",
  "overlay", "performance", "hotkeys", "transcript", "logging", "models",
] as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Rejects payloads that are not a full config before the UI renders from them. */
export function parseConfig(value: unknown): Config {
  if (!isRecord(value) || typeof value.config_version !== "number") {
    throw new Error("The app returned an unreadable settings file.");
  }
  for (const table of CONFIG_TABLES) {
    if (!isRecord(value[table])) {
      throw new Error(`The settings are missing the [${table}] section.`);
    }
  }
  return value as Config;
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

export function normalizeLang(code: string | null | undefined): string | null {
  if (!code) return null;
  const cleaned = code.replace(/<\||\|>/g, "").trim().toLowerCase().replace(/_/g, "-");
  const primary = cleaned.split("-")[0] ?? "";
  if (primary === "jpn") return "ja";
  if (primary === "kor") return "ko";
  if (primary === "zho" || primary === "chi" || primary === "cmn") return "zh";
  if (primary === "eng") return "en";
  return primary === "" ? null : primary;
}

const LANGUAGE_NAMES: Readonly<Record<string, string>> = {
  ja: "JAPANESE",
  ko: "KOREAN",
  zh: "CHINESE",
  yue: "CANTONESE",
  en: "ENGLISH",
};

export function languageLabel(code: string | null | undefined): string {
  const lang = normalizeLang(code);
  if (lang === null) return "OTHER LANGUAGE";
  return LANGUAGE_NAMES[lang] ?? lang.toUpperCase();
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

/** Characters of earlier lines translated together with each new line; 0 turns it off. */
export const WINDOW_LIMITS = { min: 0, max: 60 } as const;

export function windowCharsPatch(chars: number): ConfigPatch {
  const value = Number.isFinite(chars) ? Math.round(chars) : 0;
  return { translate: { window_chars: clamp(value, WINDOW_LIMITS.min, WINDOW_LIMITS.max) } };
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

/** A speech model choice; `model` is its manifest id, `null` when nothing is downloaded. */
export interface SpeechChoice<T extends string> {
  value: T;
  label: string;
  model: string | null;
  help: string;
}

export const STREAM_CHUNKS: readonly SpeechChoice<"160" | "480" | "960">[] = [
  { value: "160", label: "160 ms", model: "xasr-stream-160ms-int8-2026-06-05", help: "Words appear soonest, about 0.15 s before 480 ms. About twice the CPU and slightly more errors." },
  { value: "480", label: "480 ms", model: "xasr-stream-480ms-int8-2026-06-05", help: "The default. Fewest errors, medium CPU." },
  { value: "960", label: "960 ms", model: "xasr-stream-960ms-int8-2026-06-05", help: "Least CPU, about 30% less than 480 ms. Words appear about 0.2 s later." },
];

export const TRANSLATORS: readonly SpeechChoice<Config["translate"]["model"]>[] = [
  { value: "ternary", label: "Small (ternary)", model: "hy-mt2-1.8b-ternary-tq2_0", help: "The default. About 730 MB of memory and fast. Translates each line on its own." },
  { value: "q4_0", label: "Large (Q4_0)", model: "hy-mt2-1.8b-q4_0", help: "About 450 MB more memory and a little slower. Can use earlier lines as context." },
];

export const SECOND_PASSES: readonly SpeechChoice<Config["asr"]["second_pass"]>[] = [
  { value: "off", label: "Off", model: null, help: "The live Chinese is final. The English appears soonest." },
  { value: "sensevoice", label: "SenseVoice", model: "sensevoice-2024-07-17-int8", help: "Fixes some misheard words. The English appears about 0.4 s later; about 300 MB more memory." },
  { value: "qwen3", label: "Qwen3-ASR", model: "qwen3-asr-0.6b-int8-2026-03-25", help: "Fixes the most misheard words. The English appears about 1 s later with 2 threads; about 1.5 GB more memory and half a CPU core." },
];

export function modelSourcePatch(source: Config["models"]["source"]): ConfigPatch {
  return { models: { source } };
}

/** "Ctrl+Shift+P" -> "Ctrl Shift P" as the spec prints bindings. */
export function formatAccelerator(accelerator: string): string {
  return accelerator.split("+").map((part) => part.trim()).filter(Boolean).join(" ");
}

/** Kept as a JSON initializer so the Rust parity test can read it directly. */
export const DEFAULT_CONFIG: Config = {
  "config_version": 2,
  "capture": {
    "mode": "system",
    "device": "default",
    "apps": [],
    "autostart": true
  },
  "audio": {
    "normalize": true,
    "norm_attack_s": 1.0,
    "norm_release_s": 5.0,
    "norm_max_gain_db": 20.0,
    "norm_gate_dbfs": -60.0
  },
  "asr": {
    "chunk_ms": 480,
    "threads": 1,
    "pause_s": 0.8,
    "second_pass": "sensevoice",
    "second_pass_threads": 1,
    "second_pass_max_wait_s": 1.5
  },
  "filter": {
    "single_char_max_s": 0.6,
    "single_char_allow": ["对", "好", "是", "行", "不", "哦"],
    "fillers": ["呃", "嗯", "额", "uh", "um"]
  },
  "translate": {
    "server_url": "",
    "model": "ternary",
    "threads": 2,
    "ctx": 1024,
    "temperature": 0.0,
    "repeat_penalty": 1.05,
    "max_tokens_cap": 256,
    "timeout_s": 10.0,
    "queue_join_max_chars": 120,
    "skip_lag_s": 6.0,
    "window_chars": 30
  },
  "overlay": {
    "visible": true,
    "style": "bar",
    "font_px": 26,
    "background": 0.82,
    "show_source": true,
    "expire_s": 8.0,
    "panel_edge": "right",
    "panel_lines": 5,
    "flow_lines": 3,
    "live_source": true
  },
  "performance": {
    "low_priority": true
  },
  "hotkeys": {
    "move_lock": "Ctrl+Shift+L",
    "show_hide": "Ctrl+Shift+H",
    "pause": "Ctrl+Shift+P"
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
    "source": "huggingface",
    "dir": ""
  }
};
