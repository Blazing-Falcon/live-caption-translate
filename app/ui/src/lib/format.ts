import type { AudioApp, ListeningStateKind, ModelStatus, PipelineStats } from "./types";

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

export function totalCpu(stats: Pick<PipelineStats, "cpu_app_pct" | "cpu_translator_pct">): number {
  return Math.round(stats.cpu_app_pct + stats.cpu_translator_pct);
}

export function totalMemoryMb(stats: Pick<PipelineStats, "rss_app_mb" | "rss_translator_mb">): number {
  return stats.rss_app_mb + stats.rss_translator_mb;
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

export function modelsAllReady(models: readonly ModelStatus[] | null): boolean {
  return models !== null && models.length > 0 && models.every((model) => model.state === "ready");
}
