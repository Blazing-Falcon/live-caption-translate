import { DEFAULT_CONFIG, cloneConfig, type Config } from "../src/lib/settings";
import type { Clock } from "../src/lib/captions";
import type {
  AppState,
  FailReason,
  PipelineEvent,
  PipelineStats,
  TextClass,
  Timing,
} from "../src/lib/types";

interface Timer {
  id: number;
  at: number;
  fn: () => void;
}

/** Deterministic monotonic clock: timers only run when the test advances time. */
export class FakeClock implements Clock {
  time = 0;
  private seq = 0;
  private timers: Timer[] = [];

  now(): number {
    return this.time;
  }

  setTimer(fn: () => void, ms: number): unknown {
    this.seq += 1;
    this.timers.push({ id: this.seq, at: this.time + Math.max(0, ms), fn });
    return this.seq;
  }

  clearTimer(handle: unknown): void {
    this.timers = this.timers.filter((timer) => timer.id !== handle);
  }

  pending(): number {
    return this.timers.length;
  }

  advance(ms: number): void {
    const target = this.time + ms;
    for (;;) {
      const due = this.timers.filter((timer) => timer.at <= target).sort((a, b) => a.at - b.at || a.id - b.id)[0];
      if (!due) break;
      this.timers = this.timers.filter((timer) => timer.id !== due.id);
      this.time = Math.max(this.time, due.at);
      due.fn();
    }
    this.time = target;
  }
}

export const TIMING: Timing = {
  speech_end_ms: 1000,
  asr_done_ms: 1300,
  queued_ms: 1310,
  sent_ms: 1310,
  first_token_ms: 1800,
  done_ms: 2200,
  prompt_tokens: 10,
  cached_tokens: 0,
  generated_tokens: 8,
};

export function asrFinal(id: number, text: string, cls: TextClass = "chinese", lang: string | null = "zh"): PipelineEvent {
  return { type: "asr_final", id, text, class: cls, lang, start_ms: 0, end_ms: 1000, asr_ms: 300 };
}

export const delta = (id: number, text: string): PipelineEvent => ({ type: "translation_delta", id, text_so_far: text });
export const final = (id: number, text: string): PipelineEvent => ({
  type: "translation_final",
  id,
  text,
  timing: TIMING,
});
export const failed = (id: number, reason: FailReason, message = ""): PipelineEvent => ({
  type: "translation_failed",
  id,
  reason,
  message,
});
export const skipped = (id: number): PipelineEvent => ({ type: "skipped", id, reason: "catch_up" });
export const joined = (id: number, absorbed: number[], text: string, kind: "hold" | "queue" = "hold"): PipelineEvent => ({
  type: "joined",
  id,
  absorbed,
  text,
  kind,
});

export function testConfig(patch?: (config: Config) => void): Config {
  const config = cloneConfig(DEFAULT_CONFIG);
  patch?.(config);
  return config;
}

export function testState(patch: Partial<AppState> = {}): AppState {
  return {
    listening: "listening",
    source: { mode: "system", label: "Speakers (Realtek)", sample_rate: 48000, channels: 2 },
    source_state: "playing",
    engines: { vad: "ready", asr: "ready", translator: "ready" },
    models_ready: true,
    overlay_moving: false,
    overlay_visible: true,
    ...patch,
  };
}

export const STATS: PipelineStats = {
  lag_ms: 400,
  queue_depth: 1,
  held: 0,
  done_p50_ms: 1800,
  done_p95_ms: 2100,
  first_p50_ms: 700,
  skipped_total: 3,
  failed_total: 1,
  cpu_app_pct: 2.5,
  cpu_translator_pct: 38.4,
  rss_app_mb: 520,
  rss_translator_mb: 1380,
};
