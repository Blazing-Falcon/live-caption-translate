// Keep this contract aligned with lt-core.
// Utterance ids are JavaScript safe integers (0 through 2^53 - 1).
export type UtteranceId = number;
export type TextClass = "chinese" | "mixed" | "english" | "other";
export type CaptureMode = "system" | "apps";
export type JoinKind = "hold" | "queue";
export type SkipReason = "catch_up";
export type FailReason = "timeout" | "server_unavailable" | "echo" | "runaway" | "error";
export type DropReason = "empty" | "music" | "single_char";
export type EngineKind = "vad" | "asr" | "translator";
export type EngineState = "loading" | "ready" | "restarting" | "failed";
export type SourceStateKind =
  | "playing"
  | "silent"
  | "no_device"
  | "no_apps_selected"
  | "apps_not_running"
  | "unsupported";
export type ListeningStateKind = "starting" | "listening" | "paused";

// Every *_ms timestamp below uses the monotonic session clock.
export interface Timing {
  speech_end_ms: number;
  asr_done_ms: number;
  queued_ms: number;
  sent_ms: number;
  first_token_ms: number | null;
  done_ms: number;
  prompt_tokens: number;
  cached_tokens: number;
  generated_tokens: number;
}

export interface SourceInfo {
  mode: CaptureMode;
  label: string;
  sample_rate: number;
  channels: number;
}

export interface PipelineStats {
  lag_ms: number;
  queue_depth: number;
  held: number;
  done_p50_ms: number | null;
  done_p95_ms: number | null;
  first_p50_ms: number | null;
  skipped_total: number;
  failed_total: number;
  cpu_app_pct: number;
  cpu_translator_pct: number;
  rss_app_mb: number;
  rss_translator_mb: number;
}

export type PipelineEvent =
  | { type: "speech_started"; id: UtteranceId; at_ms: number }
  | { type: "asr_partial"; id: UtteranceId; text: string }
  | {
      type: "asr_final";
      id: UtteranceId;
      text: string;
      class: TextClass;
      lang: string | null;
      start_ms: number;
      end_ms: number;
      asr_ms: number;
    }
  | { type: "joined"; id: UtteranceId; absorbed: UtteranceId[]; text: string; kind: JoinKind }
  | { type: "translation_delta"; id: UtteranceId; text_so_far: string }
  | { type: "translation_final"; id: UtteranceId; text: string; timing: Timing }
  | { type: "skipped"; id: UtteranceId; reason: SkipReason }
  | { type: "translation_failed"; id: UtteranceId; reason: FailReason; message: string }
  | { type: "dropped"; id: UtteranceId; reason: DropReason }
  | { type: "source_changed"; info: SourceInfo }
  | { type: "source_state"; state: SourceStateKind; detail: string | null }
  | { type: "listening_state"; state: ListeningStateKind }
  | { type: "engine_status"; engine: EngineKind; state: EngineState; message: string | null }
  | ({ type: "stats" } & PipelineStats);

export interface AppState {
  listening: ListeningStateKind;
  source: SourceInfo | null;
  source_state: SourceStateKind;
  engines: Record<EngineKind, EngineState>;
  models_ready: boolean;
  overlay_moving: boolean;
  overlay_visible: boolean;
}

export interface AudioDevice {
  id: string;
  name: string;
  is_default: boolean;
}

export interface AudioApp {
  exe: string;
  name: string;
  pid: number;
  icon_png: string | null;
  peak: number;
  active: boolean;
  recent: boolean;
}

export interface AudioAppList {
  supported: boolean;
  reason: string | null;
  apps: AudioApp[];
}

export interface ModelStatus {
  id: string;
  name: string;
  bytes_total: number;
  bytes_done: number;
  state: "missing" | "downloading" | "paused" | "verifying" | "ready" | "corrupt";
}

export type LineState = "pending" | "streaming" | "final" | "english" | "other" | "skipped" | "failed";

export interface CaptionLine {
  id: UtteranceId;
  state: LineState;
  source: string;
  english: string;
  lang: string | null;
  reason: string | null;
  updatedAt: number;
}
