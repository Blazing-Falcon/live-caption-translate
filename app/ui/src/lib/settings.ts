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
  vad: ExtraFields & {
    engine: string;
    threshold: number;
    min_speech_s: number;
    min_silence_s: number;
    pre_roll_s: number;
    post_roll_s: number;
    soft_cut_after_s: number;
    soft_cut_prob: number;
    hard_cut_s: number;
  };
  asr: ExtraFields & {
    engine: string;
    language: "auto" | "zh" | "en" | "ja" | "ko" | "yue";
    use_itn: boolean;
    threads: number;
  };
  filter: ExtraFields & {
    drop_music: boolean;
    single_char_max_s: number;
    single_char_allow: string[];
    fillers: string[];
  };
  routing: ExtraFields & {
    english: "passthrough";
    translate_other: string[];
  };
  join: ExtraFields & {
    hold_max_chars: number;
    hold_window_s: number;
    max_segments: number;
    max_chars: number;
  };
  translate: ExtraFields & {
    engine: string;
    server_url: string;
    threads: number;
    ctx: number;
    temperature: number;
    repeat_penalty: number;
    max_tokens_cap: number;
    timeout_s: number;
    queue_join_max_chars: number;
    skip_lag_s: number;
    target: "en";
  };
  overlay: ExtraFields & {
    visible: boolean;
    style: "bar" | "panel";
    font_px: number;
    background: number;
    show_source: boolean;
    expire_s: number;
    panel_edge: "left" | "right";
    panel_lines: number;
    bar_rect?: OverlayRect;
    panel_rect?: OverlayRect;
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
} as const;

export const TRANSLATE_THREADS_MAX = 16;
export const LOW_BACKGROUND_WARNING = 0.5;

export const HOTKEY_ACTIONS = ["move_lock", "show_hide", "pause"] as const;
export type HotkeyAction = (typeof HOTKEY_ACTIONS)[number];

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export function cloneConfig(config: Config): Config {
  return structuredClone(config);
}

const CONFIG_TABLES = [
  "capture", "audio", "vad", "asr", "filter", "routing", "join", "translate",
  "overlay", "hotkeys", "transcript", "logging", "models",
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

export function isOtherLangTranslated(translateOther: readonly string[], lang: string | null | undefined): boolean {
  const wanted = normalizeLang(lang);
  if (wanted === null) return false;
  return translateOther.some((code) => normalizeLang(code) === wanted);
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

/** Keeps every enabled code the UI does not expose (anything besides the toggled one). */
export function translateOtherPatch(current: readonly string[], code: string, enabled: boolean): ConfigPatch {
  const target = normalizeLang(code) ?? code;
  const rest = current.filter((entry) => (normalizeLang(entry) ?? entry) !== target);
  return { routing: { translate_other: enabled ? [...rest, target] : rest } };
}

export function translateThreadsPatch(threads: number): ConfigPatch {
  return { translate: { threads: clamp(Math.round(threads), 0, TRANSLATE_THREADS_MAX) } };
}

export function transcriptPatch(enabled: boolean): ConfigPatch {
  return { transcript: { enabled } };
}

export function modelSourcePatch(source: Config["models"]["source"]): ConfigPatch {
  return { models: { source } };
}

/** "Ctrl+Shift+P" -> "Ctrl Shift P" as the spec prints bindings. */
export function formatAccelerator(accelerator: string): string {
  return accelerator.split("+").map((part) => part.trim()).filter(Boolean).join(" ");
}

/** Kept as a JSON initializer so the Rust parity test can read it directly. */
export const DEFAULT_CONFIG: Config = {
  "config_version": 1,
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
  "vad": {
    "engine": "silero",
    "threshold": 0.5,
    "min_speech_s": 0.25,
    "min_silence_s": 0.4,
    "pre_roll_s": 0.3,
    "post_roll_s": 0.1,
    "soft_cut_after_s": 7.0,
    "soft_cut_prob": 0.35,
    "hard_cut_s": 10.0
  },
  "asr": {
    "engine": "sensevoice",
    "language": "auto",
    "use_itn": true,
    "threads": 1
  },
  "filter": {
    "drop_music": true,
    "single_char_max_s": 0.6,
    "single_char_allow": ["对", "好", "是", "行", "不", "哦"],
    "fillers": ["呃", "嗯", "额", "uh", "um"]
  },
  "routing": {
    "english": "passthrough",
    "translate_other": []
  },
  "join": {
    "hold_max_chars": 8,
    "hold_window_s": 1.0,
    "max_segments": 3,
    "max_chars": 40
  },
  "translate": {
    "engine": "hymt2",
    "server_url": "",
    "threads": 0,
    "ctx": 1024,
    "temperature": 0.0,
    "repeat_penalty": 1.05,
    "max_tokens_cap": 256,
    "timeout_s": 10.0,
    "queue_join_max_chars": 120,
    "skip_lag_s": 6.0,
    "target": "en"
  },
  "overlay": {
    "visible": true,
    "style": "bar",
    "font_px": 26,
    "background": 0.82,
    "show_source": true,
    "expire_s": 8.0,
    "panel_edge": "right",
    "panel_lines": 5
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
