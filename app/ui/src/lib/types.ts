// Keep this contract aligned with lt-core.
// Utterance ids are JavaScript safe integers (0 through 2^53 - 1).
export type UtteranceId = number;
export type TextClass = "chinese" | "mixed" | "english" | "other";
export type CaptureMode = "system" | "apps";
export type SkipReason = "catch_up";
export type FailReason = "timeout" | "server_unavailable" | "echo" | "runaway" | "error";
export type DropReason = "empty" | "single_char";
export type EngineKind = "asr" | "translator";
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
  done_p50_ms: number | null;
  done_p95_ms: number | null;
  first_p50_ms: number | null;
  skipped_total: number;
  failed_total: number;
  cpu_app_pct: number;
  cpu_translator_pct: number;
  rss_app_mb: number;
  rss_translator_mb: number;
  cpu_system_pct: number;
  word_final_p50_ms: number | null;
}

export type CutReason = "pause" | "discontinuity" | "end" | "commit";

export type SecondPassOutcome = "used" | "mismatch" | "late" | "failed";

export interface SecondPass {
  outcome: SecondPassOutcome;
  live_text?: string;
  window_text?: string;
  window_start_ms: number;
  window_end_ms: number;
  distance?: number;
  decode_ms?: number;
  refine_ms: number;
}

export type PipelineEvent =
  | { type: "speech_started"; id: UtteranceId; at_ms: number }
  | { type: "asr_partial"; id: UtteranceId; text: string; class: TextClass; end_ms: number }
  | {
      type: "asr_final";
      id: UtteranceId;
      text: string;
      class: TextClass;
      lang: string | null;
      start_ms: number;
      end_ms: number;
      asr_ms: number;
      cut: CutReason;
      /** Mean token log-probability; only engines that give one send it. */
      logprob?: number;
      /** Absent when the second pass is off. `text` is already the final text. */
      second_pass?: SecondPass;
    }
  | { type: "joined"; id: UtteranceId; absorbed: UtteranceId[]; text: string }
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
  /** Logical CPUs of this PC: the most threads a setting can use. */
  logical_cpus: number;
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

export type ModelState = "missing" | "downloading" | "paused" | "verifying" | "ready" | "corrupt";
export type ModelSource = "huggingface" | "modelscope";
export type FolderKind = "transcripts" | "logs" | "models";

export interface ModelStatus {
  id: string;
  name: string;
  bytes_total: number;
  bytes_done: number;
  state: ModelState;
  /** Not needed to listen: a speech model that downloads only when picked on the Performance page. */
  optional: boolean;
}

/** Payload of `models://progress`: the same rows as models_status. */
export type ModelProgress = ModelStatus;

export interface HotkeyError {
  action: string;
  accelerator: string;
  message: string;
}

export interface OverlayModePayload {
  moving: boolean;
}

export interface OverlayVisiblePayload {
  visible: boolean;
}

export type LineState = "live" | "pending" | "streaming" | "final" | "english" | "other" | "skipped" | "failed";

export interface CaptionLine {
  id: UtteranceId;
  state: LineState;
  /** Chinese so far (live), then the committed Chinese. */
  source: string;
  /** Streamed translation so far, then the final translation. */
  english: string;
  lang: string | null;
  reason: string | null;
  updatedAt: number;
}
