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
