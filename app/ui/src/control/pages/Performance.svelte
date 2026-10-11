<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { reconcileChecked } from "../../lib/reconcile";
  import {
    formatBytes,
    formatMemory,
    formatSeconds,
    modeStatusText,
    modelsPercent,
    totalCpu,
    totalMemoryMb,
  } from "../../lib/format";
  import {
    CAPTION_SPEEDS,
    STREAM_CHUNKS,
    TRANSLATE_THREADS_MAX,
    VAD_ENGINES,
    asrPatch,
    latencyPatch,
    transcriptPatch,
    translateThreadsPatch,
    translateWindowPatch,
    vadPatch,
    type Config,
    type ConfigPatch,
    type SpeechChoice,
  } from "../../lib/settings";
  import type { ModelStatus } from "../../lib/types";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { stats, busy, appState, models, config: liveConfig } = session;

  const threadOptions = Array.from({ length: TRANSLATE_THREADS_MAX }, (_, index) => index + 1);

  const speed = $derived(CAPTION_SPEEDS.find((option) => option.value === config.latency.mode) ?? CAPTION_SPEEDS[0]);
  // Stats arrive about once a second; until the first one the state snapshot already knows the mode.
  const now = $derived(
    $stats
      ? { mode: $stats.mode, reason: $stats.mode_reason }
      : $appState
        ? { mode: $appState.mode, reason: $appState.mode_reason }
        : null,
  );
  const draftModel = $derived($models?.find((model) => model.id === api.DRAFT_MODEL_ID));
  const draftDownloading = $derived(draftModel?.state === "downloading" || draftModel?.state === "verifying");
  // Both draft-related switches mean nothing when the speed they belong to is not chosen.
  const stepDownDisabled = $derived(config.latency.mode === "light" || config.latency.mode === "off");
  const splitLongDisabled = $derived(config.latency.mode === "off");
  const hasDraftServer = $derived($stats !== null && ($stats.rss_draft_mb > 0 || $stats.cpu_draft_pct > 0));

  async function chooseSpeed(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const chosen = CAPTION_SPEEDS.find((option) => option.value === select.value);
    if (!chosen) {
      select.value = config.latency.mode;
      return;
    }
    const saved = await session.save("latency.mode", latencyPatch({ mode: chosen.value }));
    if (!saved) select.value = config.latency.mode;
  }

  async function toggleLatency(
    event: Event,
    key: "low_priority" | "step_down" | "split_long",
  ): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save(`latency.${key}`, latencyPatch({ [key]: input.checked }));
    reconcileChecked(input, get(liveConfig)?.latency[key] ?? config.latency[key]);
  }

  async function downloadDraft(): Promise<void> {
    if (get(busy).has("models.download") || draftDownloading) return;
    await session.act("models.download", () => api.modelsDownload(config.models.source, [api.DRAFT_MODEL_ID]));
  }

  async function chooseThreads(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("translate.threads", translateThreadsPatch(Number(select.value)));
    if (!saved) select.value = String(config.translate.threads);
  }

  async function toggleWindow(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("translate.window_chars", translateWindowPatch(input.checked));
    reconcileChecked(input, (get(liveConfig)?.translate.window_chars ?? config.translate.window_chars) > 0);
  }

  async function toggleTranscript(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("transcript.enabled", transcriptPatch(input.checked));
    reconcileChecked(input, get(liveConfig)?.transcript.enabled ?? config.transcript.enabled);
  }

  interface SpeechRow {
    key: "vad.engine" | "asr.stream_chunk_ms";
    label: string;
    value: string;
    choices: readonly SpeechChoice<string>[];
    patch: (value: string) => ConfigPatch;
  }

  const speechRows = $derived<SpeechRow[]>([
    {
      key: "vad.engine",
      label: "Voice detection",
      value: config.vad.engine,
      choices: VAD_ENGINES,
      patch: (value) => vadPatch({ engine: value as Config["vad"]["engine"] }),
    },
    {
      key: "asr.stream_chunk_ms",
      label: "Streaming chunk",
      value: String(config.asr.stream_chunk_ms),
      choices: STREAM_CHUNKS,
      patch: (value) => asrPatch({ stream_chunk_ms: Number(value) as Config["asr"]["stream_chunk_ms"] }),
    },
  ]);

  /** A choice whose model is downloading; it is saved when the model is ready. */
  let waiting = $state<{ key: SpeechRow["key"]; value: string; model: string; patch: ConfigPatch } | null>(null);

  const modelOf = (id: string | null): ModelStatus | undefined =>
    id === null ? undefined : $models?.find((model) => model.id === id);
  const ready = (choice: SpeechChoice<string>): boolean => choice.model === null || modelOf(choice.model)?.state === "ready";
  const optionLabel = (choice: SpeechChoice<string>): string => {
    const model = modelOf(choice.model);
    return ready(choice) || model === undefined ? choice.label : `${choice.label} (download ${formatBytes(model.bytes_total)})`;
  };

  $effect(() => {
    const pending = waiting;
    if (pending === null || modelOf(pending.model)?.state !== "ready") return;
    waiting = null;
    void session.save(pending.key, pending.patch);
  });

  async function downloadModel(model: string): Promise<void> {
    if (get(busy).has("models.download")) return;
    await session.act("models.download", () => api.modelsDownload(config.models.source, [model]));
  }

  async function chooseSpeech(row: SpeechRow, event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const chosen = row.choices.find((choice) => choice.value === select.value);
    if (!chosen) {
      select.value = row.value;
      return;
    }
    if (chosen.model !== null && !ready(chosen)) {
      // Listening keeps the current model until the new one is on disk.
      select.value = row.value;
      waiting = { key: row.key, value: chosen.value, model: chosen.model, patch: row.patch(chosen.value) };
      await downloadModel(chosen.model);
      return;
    }
    if (waiting?.key === row.key) waiting = null;
    const saved = await session.save(row.key, row.patch(chosen.value));
    if (!saved) select.value = row.value;
  }

  function speechStatus(row: SpeechRow): { text: string; download: string | null } {
    if (waiting?.key === row.key) {
      const choice = row.choices.find((item) => item.value === waiting?.value);
      const model = modelOf(waiting.model);
      const name = choice?.label ?? waiting.value;
      if (model?.state === "downloading" || model?.state === "verifying") {
        return { text: `Downloading ${name}: ${modelsPercent(model)}%. It is used when the download is done.`, download: null };
      }
      return { text: `${name} is not downloaded.`, download: waiting.model };
    }
    if (row.key === "vad.engine") {
      return { text: "X-ASR streaming finds the pauses itself. Voice detection only places the cut between two lines.", download: null };
    }
    const current = row.choices.find((choice) => choice.value === row.value);
    if (current && !ready(current) && current.model !== null) {
      return { text: `${current.label} is not downloaded. Listening cannot start without it.`, download: current.model };
    }
    return { text: current?.help ?? "", download: null };
  }

  const openTranscripts = async (): Promise<void> => {
    if (get(busy).has("open.transcripts")) return;
    await session.act("open.transcripts", () => api.openFolder("transcripts"));
  };
</script>

<section class="page" aria-labelledby="performance-title">
  <h2 id="performance-title" class="section-title">Performance</h2>

  <div class="stack">
    <label class="field">
      <span>Caption speed</span>
      <select class="select" value={config.latency.mode} aria-describedby="caption-speed-help" onchange={chooseSpeed}>
        {#each CAPTION_SPEEDS as option (option.value)}
          <option value={option.value}>{option.label}</option>
        {/each}
      </select>
    </label>
    <p id="caption-speed-help" class="help small" data-testid="caption-speed-help">{speed?.help}</p>
    {#if now}
      <div class="status-row">
        <p class="help" aria-live="polite" data-testid="mode-status">{modeStatusText(now.mode, now.reason)}</p>
        {#if now.reason === "draft_unavailable"}
          <button type="button" class="btn" aria-disabled={$busy.has("models.download") || draftDownloading} onclick={downloadDraft}>
            Download (480 MB)
          </button>
        {/if}
      </div>
    {/if}
  </div>

  <div class="stack">
    <div class="option">
      <label class="check-row">
        <input
          type="checkbox"
          checked={config.latency.low_priority}
          aria-describedby="help-low-priority"
          onchange={(event) => toggleLatency(event, "low_priority")}
        />
        <span>Give games priority</span>
      </label>
      <p id="help-low-priority" class="help small">
        Runs translation at low priority so games stay smooth. Off: English can appear slightly sooner, but uses more CPU.
      </p>
    </div>
    <div class="option">
      <label class="check-row">
        <input
          type="checkbox"
          checked={config.latency.step_down}
          disabled={stepDownDisabled}
          aria-describedby="help-step-down"
          onchange={(event) => toggleLatency(event, "step_down")}
        />
        <span>Slow down when the PC is busy</span>
      </label>
      <p id="help-step-down" class="help small">Switches to Light while the PC is busy and back when it is free.</p>
    </div>
    <div class="option">
      <label class="check-row">
        <input
          type="checkbox"
          checked={config.latency.split_long}
          disabled={splitLongDisabled}
          aria-describedby="help-split-long"
          onchange={(event) => toggleLatency(event, "split_long")}
        />
        <span>Split long sentences</span>
      </label>
      <p id="help-split-long" class="help small">Translates long sentences in parts when the speaker does not pause.</p>
    </div>
    <div class="option">
      <label class="check-row">
        <input
          type="checkbox"
          checked={config.translate.window_chars > 0}
          aria-describedby="help-window"
          data-testid="translate-window"
          onchange={toggleWindow}
        />
        <span>Translate with the previous lines</span>
      </label>
      <p id="help-window" class="help small">
        Translates each line together with the line or two before it, so the translator knows what came before. Only the new line's English is shown. Each line's English appears a little later.
      </p>
    </div>
  </div>

  <div class="stack">
    <h3 class="section-title">Speech recognition</h3>
    {#each speechRows as row (row.key)}
      {@const status = speechStatus(row)}
      <div class="option">
        <label class="field">
          <span>{row.label}</span>
          <select
            class="select"
            value={row.value}
            aria-describedby={`help-${row.key}`}
            data-testid={`speech-${row.key}`}
            onchange={(event) => chooseSpeech(row, event)}
          >
            {#each row.choices as choice (choice.value)}
              <option value={choice.value}>{optionLabel(choice)}</option>
            {/each}
          </select>
        </label>
        <div class="status-row">
          <p id={`help-${row.key}`} class="help small" aria-live="polite">{status.text}</p>
          {#if status.download}
            <button
              type="button"
              class="btn"
              aria-disabled={$busy.has("models.download")}
              onclick={() => status.download && downloadModel(status.download)}
            >
              Download
            </button>
          {/if}
        </div>
      </div>
    {/each}
  </div>

  <dl class="stat-grid">
    <div class="stat">
      <dt>Word to first English</dt>
      <dd data-testid="stat-first">{formatSeconds($stats?.word_first_p50_ms)}</dd>
      <span class="note">median, last minute</span>
    </div>
    <div class="stat">
      <dt>Word to final English</dt>
      <dd data-testid="stat-final">{formatSeconds($stats?.word_final_p50_ms)}</dd>
      <span class="note">median, last minute</span>
    </div>
    <div class="stat">
      <dt>Waiting to translate</dt>
      <dd data-testid="stat-queue">{$stats ? $stats.queue_depth : "—"}</dd>
      <span class="note">skipped this session: {$stats ? $stats.skipped_total : "—"}</span>
    </div>
    <div class="stat">
      <dt>Memory</dt>
      <dd data-testid="stat-memory">{$stats ? formatMemory(totalMemoryMb($stats)) : "—"}</dd>
      <span class="note">
        CPU {$stats ? `${totalCpu($stats)}%` : "—"}, app and {hasDraftServer ? "translators" : "translator"}
      </span>
    </div>
  </dl>

  <label class="field">
    <span>Translator threads</span>
    <select
      class="select"
      value={String(config.translate.threads)}
      onchange={chooseThreads}
    >
      <option value="0">Automatic</option>
      {#each threadOptions as count (count)}
        <option value={String(count)}>{count}</option>
      {/each}
    </select>
  </label>

  <div class="stack">
    <label class="check-row">
      <input
        type="checkbox"
        checked={config.transcript.enabled}
        onchange={toggleTranscript}
      />
      <span>Save a transcript of each listening session</span>
    </label>
    <div>
      <button type="button" class="btn" aria-disabled={$busy.has("open.transcripts")} onclick={openTranscripts}>
        Open transcript folder
      </button>
    </div>
  </div>
</section>

<style>
  .status-row {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-height: var(--control-h);
  }

  .option {
    display: flex;
    flex-direction: column;
  }

  .option .help {
    padding-left: 26px;
  }
</style>
