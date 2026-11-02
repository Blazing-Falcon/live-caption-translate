/** The control window's plain-language status lines, kept apart from markup so tests can read them. */
import { formatAccelerator, type Config } from "./settings";
import type { AppState, PipelineStats } from "./types";
import { formatMemory, formatSeconds, totalCpu, totalMemoryMb } from "./format";

export type Engine = Config["translate"]["engine"];
export type OnlineEngine = Exclude<Engine, "local" | "openai">;

export const ONLINE_SERVICES: readonly { value: OnlineEngine; label: string }[] = [
  { value: "microsoft", label: "Microsoft" },
  { value: "google", label: "Google" },
  { value: "google_beta", label: "Google Beta" },
  { value: "bing", label: "Bing" },
  { value: "youdao", label: "Youdao" },
];

/** Translator families on the Translation page and in the first-start wizard. */
export type TranslatorKind = "online" | "local" | "server";

export function translatorKind(engine: Engine): TranslatorKind {
  return engine === "local" ? "local" : engine === "openai" ? "server" : "online";
}

export function serviceName(engine: Engine): string {
  if (engine === "local") return "This PC";
  if (engine === "openai") return "Your server";
  return ONLINE_SERVICES.find((service) => service.value === engine)?.label ?? engine;
}

/** The services a keyless engine falls back to, in the words the window uses. */
export function backupNames(engine: Engine): string {
  if (engine === "youdao") return "Microsoft";
  if (engine === "google" || engine === "bing") return "Google and Microsoft";
  return "Google";
}

export function translatorLabel(engine: Engine): string {
  const kind = translatorKind(engine);
  return kind === "online" ? `${serviceName(engine)} · online` : kind === "local" ? "On this PC" : "Your own server";
}

/** Who sees the Chinese text. Audio and screenshots never leave the PC. */
export function privacyText(config: Config): { headline: string; detail: string } {
  const kind = translatorKind(config.translate.engine);
  if (kind === "local") return { headline: "Everything stays on this PC.", detail: "Audio, text and screenshots are never sent anywhere." };
  if (kind === "server") return { headline: "Your server gets the Chinese text.", detail: "Audio and screenshots stay on this PC." };
  const name = serviceName(config.translate.engine);
  const backup = config.translate.fallback ? ` If ${name} doesn't answer, ${backupNames(config.translate.engine)} may see the text too.` : "";
  return { headline: `${name} gets the Chinese text to translate.`, detail: `Audio and screenshots stay on this PC.${backup}` };
}

export type HomeMood = "listening" | "ready" | "paused" | "trouble";

export interface HomeStatus {
  mood: HomeMood;
  title: string;
  detail: string;
}

export function homeStatus(state: AppState | null, config: Config): HomeStatus {
  if (state && (state.engines.translator === "failed" || state.engines.asr === "failed")) {
    const what = state.engines.translator === "failed" ? "The translator" : "Speech recognition";
    return { mood: "trouble", title: `${what} stopped working`, detail: "Pause and start again. If it keeps happening, check the Translation page." };
  }
  const listening = state?.listening ?? "starting";
  if (listening === "paused") {
    const key = formatAccelerator(config.hotkeys.pause);
    return {
      mood: "paused",
      title: "Paused",
      detail: key ? `Nothing is being heard. Press Start or ${config.hotkeys.pause} to continue.` : "Nothing is being heard. Press Start to continue.",
    };
  }
  if (listening === "starting") return { mood: "ready", title: "Getting ready…", detail: "Loading speech recognition and the translator." };
  const label = state?.source?.label.trim();
  const target = config.capture.mode === "apps" ? label || "the apps you picked" : "your PC";
  return { mood: "listening", title: `Listening to ${target}`, detail: "Chinese speech becomes English captions." };
}

export interface HealthRow {
  label: string;
  value: string;
  tone: "plain" | "good" | "warn";
}

export function healthRows(stats: PipelineStats | null, listening: boolean, local: boolean): HealthRow[] {
  if (!stats || !listening) return [];
  const delay = stats.word_final_p50_ms;
  const skipped = stats.skipped_total;
  const memory = totalMemoryMb(stats);
  return [
    { label: "English shows up", value: delay === null ? "Waiting for the first line" : `about ${formatSeconds(delay)} after the words`, tone: "plain" },
    {
      label: "Keeping up",
      value: skipped === 0 ? "Yes · no lines skipped" : `Skipped ${skipped} ${skipped === 1 ? "line" : "lines"} to catch up`,
      tone: skipped === 0 ? "good" : "warn",
    },
    { label: "Using", value: `${totalCpu(stats)}% CPU · ${formatMemory(memory)} memory${local ? " with the translator" : ""}`, tone: "plain" },
  ];
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** "1 Nov 2026, 21:12" in the time zone the session was recorded in. */
export function sessionTime(startedAt: string): string {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(startedAt);
  if (!match) return startedAt || "Unknown time";
  const [, year, month, day, hour, minute] = match;
  return `${Number(day)} ${MONTHS[Number(month) - 1] ?? month} ${year}, ${hour}:${minute}`;
}

/** "21:12:05" from a line's wall time. */
export function lineTime(wall: string): string {
  return /T(\d{2}:\d{2}:\d{2})/.exec(wall)?.[1] ?? "";
}
