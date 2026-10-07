<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { reconcileChecked } from "../../lib/reconcile";
  import { formatMemory, formatSeconds, totalCpu, totalMemoryMb } from "../../lib/format";
  import { TRANSLATE_THREADS_MAX, transcriptPatch, translateThreadsPatch, type Config } from "../../lib/settings";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { stats, busy, config: liveConfig } = session;

  const threadOptions = Array.from({ length: TRANSLATE_THREADS_MAX }, (_, index) => index + 1);

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
  <dl class="stat-grid">
    <div class="stat">
      <dt>Speech end to English</dt>
      <dd data-testid="stat-median">{formatSeconds($stats?.done_p50_ms)}</dd>
      <span class="note">median, last 50 lines</span>
    </div>
    <div class="stat">
      <dt>Waiting to translate</dt>
      <dd data-testid="stat-queue">{$stats ? $stats.queue_depth : "—"}</dd>
      <span class="note">skipped this session: {$stats ? $stats.skipped_total : "—"}</span>
    </div>
    <div class="stat">
      <dt>Memory</dt>
      <dd data-testid="stat-memory">{$stats ? formatMemory(totalMemoryMb($stats)) : "—"}</dd>
      <span class="note">CPU {$stats ? `${totalCpu($stats)}%` : "—"}, app and translator</span>
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
