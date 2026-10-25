<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { reconcileChecked } from "../../lib/reconcile";
  import {
    formatMemory,
    formatSeconds,
    totalCpu,
    totalMemoryMb,
  } from "../../lib/format";
  import {
    asrPatch,
    performancePatch,
    threadsPatch,
    transcriptPatch,
    type Config,
  } from "../../lib/settings";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { stats, busy, appState, config: liveConfig } = session;

  // Up to the PC's logical CPUs; a larger saved value stays selectable.
  const threadOptions = (current: number): number[] =>
    Array.from({ length: Math.max($appState?.logical_cpus ?? 1, current) }, (_, index) => index + 1);

  async function toggleLowPriority(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("performance.low_priority", performancePatch({ low_priority: input.checked }));
    reconcileChecked(input, get(liveConfig)?.performance.low_priority ?? config.performance.low_priority);
  }

  async function chooseThreads(event: Event, table: "asr" | "translate"): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save(`${table}.threads`, threadsPatch(table, Number(select.value)));
    if (!saved) select.value = String(config[table].threads);
  }

  async function chooseSecondPassThreads(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("asr.second_pass_threads", asrPatch({ second_pass_threads: Number(select.value) }));
    if (!saved) select.value = String(config.asr.second_pass_threads);
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
    <h3 class="section-title">Speech recognition</h3>
    <label class="field">
      <span>Recognizer threads</span>
      <select
        class="select"
        value={String(config.asr.threads)}
        data-testid="asr-threads"
        onchange={(event) => chooseThreads(event, "asr")}
      >
        {#each threadOptions(config.asr.threads) as count (count)}
          <option value={String(count)}>{count}</option>
        {/each}
      </select>
    </label>
    <label class="field">
      <span>Correction threads</span>
      <select
        class="select"
        value={String(config.asr.second_pass_threads)}
        data-testid="asr-second-pass-threads"
        onchange={chooseSecondPassThreads}
      >
        {#each threadOptions(config.asr.second_pass_threads) as count (count)}
          <option value={String(count)}>{count}</option>
        {/each}
      </select>
    </label>
    <p class="help small">
      Each finished line is read again by X-ASR offline with the speech before it, and the corrected Chinese replaces the live text before translation.
    </p>
  </div>

  <div class="stack">
    <h3 class="section-title">Translation</h3>
    <div class="field">
      <span>Translation model</span>
      <span class="model-name" data-testid="translate-model">Tencent Hy-MT2 1.8B (1.25-bit)</span>
    </div>
    <label class="field">
      <span>Translator threads</span>
      <select
        class="select"
        value={String(config.translate.threads)}
        data-testid="translate-threads"
        onchange={(event) => chooseThreads(event, "translate")}
      >
        {#each threadOptions(config.translate.threads) as count (count)}
          <option value={String(count)}>{count}</option>
        {/each}
      </select>
    </label>
    <div class="option">
      <label class="check-row">
        <input
          type="checkbox"
          checked={config.performance.low_priority}
          aria-describedby="help-low-priority"
          onchange={toggleLowPriority}
        />
        <span>Give games priority</span>
      </label>
      <p id="help-low-priority" class="help small">
        Runs translation at low priority so games stay smooth. Off: English can appear slightly sooner, but uses more CPU.
      </p>
    </div>
  </div>

  <dl class="stat-grid">
    <div class="stat">
      <dt>Word to English</dt>
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
        CPU {$stats ? `${totalCpu($stats)}%` : "—"}, app and translation server
      </span>
    </div>
  </dl>

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
  .option {
    display: flex;
    flex-direction: column;
  }

  .option .check-row + .help {
    padding-left: 26px;
  }

  .model-name {
    font-size: var(--text-ui);
    color: var(--c-text);
  }
</style>
