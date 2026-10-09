import type { Page } from "@playwright/test";
import type { Boot } from "../harness/boot";
import type { CutReason, PipelineEvent } from "../../src/lib/types";

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
