<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { reconcileChecked } from "../../lib/reconcile";
  import { formatMemory, formatSeconds, modeStatusText, totalCpu, totalMemoryMb } from "../../lib/format";
  import {
    CAPTION_SPEEDS,
    TRANSLATE_THREADS_MAX,
    latencyPatch,
    transcriptPatch,
    translateThreadsPatch,
    type Config,
  } from "../../lib/settings";

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

  async function toggleTranscript(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("transcript.enabled", transcriptPatch(input.checked));
    reconcileChecked(input, get(liveConfig)?.transcript.enabled ?? config.transcript.enabled);
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
