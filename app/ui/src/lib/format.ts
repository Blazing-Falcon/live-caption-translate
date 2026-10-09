import type { AudioApp, EffectiveMode, ListeningStateKind, ModeReason, ModelStatus, PipelineStats } from "./types";

const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;

export function formatBytes(bytes: number): string {
  if (bytes >= GB) return `${(bytes / GB).toFixed(2)} GB`;
  if (bytes >= MB) {
    const value = bytes / MB;
    return `${value >= 10 ? Math.round(value) : value.toFixed(1)} MB`;
  }
  if (bytes >= KB) return `${Math.round(bytes / KB)} KB`;
  return `${Math.max(0, Math.round(bytes))} B`;
}

export function formatMemory(megabytes: number): string {
  return megabytes >= 1024 ? `${(megabytes / 1024).toFixed(1)} GB` : `${Math.round(megabytes)} MB`;
}

export function formatSeconds(ms: number | null | undefined): string {
  return ms === null || ms === undefined ? "—" : `${(ms / 1000).toFixed(1)} s`;
}

/** Peak 0..1 as a readable level; exactly 0 means no signal. */
export function levelText(app: Pick<AudioApp, "peak" | "active">): string {
  if (!app.active) return "not playing";
  if (!(app.peak > 0)) return "silent";
  return `${Math.round(20 * Math.log10(Math.min(1, app.peak)))} dB`;
}

/** App plus both translator servers (the draft server is 0 when it is not running). */
export function totalCpu(stats: Pick<PipelineStats, "cpu_app_pct" | "cpu_translator_pct" | "cpu_draft_pct">): number {
  return Math.round(stats.cpu_app_pct + stats.cpu_translator_pct + stats.cpu_draft_pct);
}

export function totalMemoryMb(stats: Pick<PipelineStats, "rss_app_mb" | "rss_translator_mb" | "rss_draft_mb">): number {
  return stats.rss_app_mb + stats.rss_translator_mb + stats.rss_draft_mb;
}

const MODE_NAMES: Record<EffectiveMode, string> = { continuous: "Continuous", light: "Light", off: "Off" };

/** The Performance page status line. */
export function modeStatusText(mode: EffectiveMode, reason: ModeReason | null): string {
  const name = `Now: ${MODE_NAMES[mode]}`;
  if (mode !== "light") return name;
  switch (reason) {
    case "cpu":
      return `${name}, because the PC is busy`;
    case "lag":
      return `${name}, because translation fell behind`;
    case "draft_unavailable":
      return `${name}, because the draft model is not downloaded`;
    default:
      return name;
  }
}

/** The first-run list names the draft model by what it does for the user and its size. */
export function modelTitle(model: Pick<ModelStatus, "id" | "name">): string {
  return model.id === "lmt-60-0.6b-q4_k_m" ? "Faster captions (LMT-60 0.6B, 480 MB)" : model.name;
}

export function footerText(listening: ListeningStateKind | undefined, stats: PipelineStats | null): string {
  if (listening === "paused") return "paused";
  if (!stats) return "speech→English — median · queue — · CPU —";
  return `speech→English ${formatSeconds(stats.done_p50_ms)} median · queue ${stats.queue_depth} · CPU ${totalCpu(stats)}%`;
}

export function modelsPercent(model: Pick<ModelStatus, "bytes_done" | "bytes_total">): number {
  if (model.bytes_total <= 0) return 0;
  return Math.min(100, Math.round((model.bytes_done / model.bytes_total) * 100));
}

/** Every model needed to listen is ready; the optional draft model never blocks first run. */
export function modelsAllReady(models: readonly ModelStatus[] | null): boolean {
  if (models === null) return false;
  const required = models.filter((model) => !model.optional);
  return required.length > 0 && required.every((model) => model.state === "ready");
}
