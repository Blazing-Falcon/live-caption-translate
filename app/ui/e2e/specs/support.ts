import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import type { Boot } from "../harness/boot";
import type { CutReason, ModelStatus, PipelineEvent, PipelineStats } from "../../src/lib/types";

export const SCREENSHOTS = fileURLToPath(new URL("../../test-results/screenshots/", import.meta.url));
export const MOCKUPS = fileURLToPath(new URL("../mockups/", import.meta.url));

mkdirSync(SCREENSHOTS, { recursive: true });

export interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

export async function open(page: Page, kind: "overlay" | "control", boot: Boot = {}, query = ""): Promise<string[]> {
  const external: string[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (/^https?:$/.test(url.protocol) && url.hostname !== "127.0.0.1") external.push(request.url());
  });
  await page.addInitScript((value) => {
    window.__LT_BOOT = value;
  }, boot);
  await page.goto(`/e2e/harness/${kind}.html${query}`);
  await page.waitForSelector(kind === "overlay" ? ".overlay-root" : ".shell, .first-run, .loading");
  return external;
}

export async function emitEvent(page: Page, event: PipelineEvent): Promise<void> {
  await page.evaluate((payload) => window.__lt.emit("pipeline://event", payload), event);
}

export async function emitAll(page: Page, events: PipelineEvent[]): Promise<void> {
  for (const event of events) await emitEvent(page, event);
}

export async function emitNamed(page: Page, name: string, payload: unknown): Promise<void> {
  await page.evaluate(([n, p]) => window.__lt.emit(n as string, p), [name, payload] as const);
}

export function calls(page: Page, cmd?: string): Promise<Call[]> {
  return page.evaluate((name) => {
    const all = window.__lt.backend.calls as Call[];
    return name ? all.filter((call) => call.cmd === name) : all;
  }, cmd);
}

/** Places the transparent overlay page over a fake video backdrop like the mockup boards do. */
export async function placeBar(page: Page): Promise<void> {
  await page.addStyleTag({
    content: `
      html { background: #2a2c30; }
      #app { padding: 0 160px 56px; }
      body::before { content: "[Video or stream playing in any app]"; position: fixed; inset: 0; display: flex;
        align-items: center; justify-content: center; font-size: 15px; color: #8a8d93; }
    `,
  });
}

export async function placePanel(page: Page): Promise<void> {
  await page.addStyleTag({
    content: `
      html { background: #2a2c30; }
      #app { padding: 24px 24px 24px 844px; }
      body::before { content: "[Video or stream playing in any app]"; position: fixed; inset: 0 460px 0 0; display: flex;
        align-items: center; justify-content: center; font-size: 15px; color: #8a8d93; }
    `,
  });
}

export const NOW = {
  asr: (id: number, text: string, cls: "chinese" | "mixed" | "english" | "other" = "chinese", lang: string | null = "zh", cut: CutReason = "pause"): PipelineEvent => ({
    type: "asr_final", id, text, class: cls, lang, start_ms: 0, end_ms: 1000, asr_ms: 300, cut,
  }),
  partial: (id: number, text: string, cls: "chinese" | "mixed" | "english" | "other" = "chinese"): PipelineEvent => ({
    type: "asr_partial", id, text, class: cls, end_ms: 1000,
  }),
  draft: (id: number, text: string, rev = 1): PipelineEvent => ({ type: "translation_draft", id, rev, text, end_ms: 1000 }),
  skipped: (id: number): PipelineEvent => ({ type: "skipped", id, reason: "catch_up" }),
  delta: (id: number, text_so_far: string): PipelineEvent => ({ type: "translation_delta", id, text_so_far }),
  final: (id: number, text: string): PipelineEvent => ({
    type: "translation_final", id, text,
    timing: { speech_end_ms: 1, asr_done_ms: 2, queued_ms: 3, sent_ms: 4, first_token_ms: 5, done_ms: 6, prompt_tokens: 1, cached_tokens: 0, generated_tokens: 1 },
  }),
};

export const MB = 1024 * 1024;

export const STATS_V2: PipelineStats = {
  lag_ms: 400,
  queue_depth: 1,
  held: 0,
  done_p50_ms: 1800,
  done_p95_ms: 2100,
  first_p50_ms: 700,
  skipped_total: 0,
  failed_total: 0,
  cpu_app_pct: 4.5,
  cpu_translator_pct: 38.4,
  rss_app_mb: 520,
  rss_translator_mb: 1380,
  mode: "continuous",
  mode_reason: "auto",
  cpu_system_pct: 41.5,
  cpu_draft_pct: 55,
  rss_draft_mb: 610,
  word_first_p50_ms: 1040,
  word_final_p50_ms: 2870,
  drafts_total: 48,
  drafts_failed: 1,
};

export const READY: ModelStatus[] = [
  { id: "vad", name: "Voice detection", bytes_total: 2 * MB, bytes_done: 2 * MB, optional: false, recommended: false, state: "ready" },
  { id: "asr", name: "Speech recognition", bytes_total: 239 * MB, bytes_done: 239 * MB, optional: false, recommended: false, state: "ready" },
  { id: "mt", name: "Translation", bytes_total: 1105 * MB, bytes_done: 1105 * MB, optional: false, recommended: false, state: "ready" },
];

export const DRAFT: ModelStatus = {
  id: "lmt-60-0.6b-q4_k_m",
  name: "Faster captions (LMT-60 0.6B, Q4_K_M)",
  bytes_total: 484_220_000,
  bytes_done: 0,
  optional: true,
  recommended: false,
  state: "missing",
};

