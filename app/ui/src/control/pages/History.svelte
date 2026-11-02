<script lang="ts">
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { transcriptPatch, type Config } from "../../lib/settings";
  import Button from "../../lib/ui/Button.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";
  import Switch from "../../lib/ui/Switch.svelte";
  import Tag from "../../lib/ui/Tag.svelte";
  import type { SavedLine, SavedSession } from "../../lib/types";
  import { lineTime, sessionTime } from "../../lib/wording";
  import { onMount } from "svelte";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { busy } = session;

  const KEEP = [
    { value: 7, label: "7 days" },
    { value: 30, label: "30 days" },
    { value: 90, label: "90 days" },
    { value: 0, label: "Forever" },
  ];
  const keepOptions = $derived(
    KEEP.some((option) => option.value === config.transcript.retention_days)
      ? KEEP
      : [...KEEP, { value: config.transcript.retention_days, label: `${config.transcript.retention_days} days` }],
  );

  let sessions = $state<SavedSession[] | null>(null);
  let reading = $state<{ session: SavedSession; lines: SavedLine[] } | null>(null);

  onMount(async () => {
    sessions = await session.act("transcripts.list", () => api.listTranscripts());
  });

  async function openSession(saved: SavedSession): Promise<void> {
    const lines = await session.act("transcripts.read", () => api.readTranscript(saved.file));
    if (lines) reading = { session: saved, lines };
  }

  async function chooseKeep(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("transcript.retention_days", { transcript: { retention_days: Number(select.value) } });
    if (!saved) select.value = String(config.transcript.retention_days);
  }
</script>

<section class="page" aria-labelledby="history-title">
  <div class="page-head">
    <h2 id="history-title">History</h2>
    <p>A saved transcript of each listening session, Chinese and English.</p>
  </div>

  <Section>
    <Row title="Save transcripts" help="Saved on this PC only.">
      {#snippet control()}
        <Switch label="Save transcripts" checked={config.transcript.enabled} onchange={(enabled) => session.save("transcript.enabled", transcriptPatch(enabled))} />
      {/snippet}
    </Row>
    <Row title="Keep them for" help="Older ones are deleted when the app starts.">
      {#snippet control()}
        <select class="field" aria-label="Keep them for" value={String(config.transcript.retention_days)} onchange={chooseKeep}>
          {#each keepOptions as option (option.value)}<option value={String(option.value)}>{option.label}</option>{/each}
        </select>
      {/snippet}
    </Row>
    <Row title="Transcript folder">
      {#snippet control()}
        <Button size="small" busy={$busy.has("open.transcripts")} onclick={() => session.act("open.transcripts", () => api.openFolder("transcripts"))}>Open folder</Button>
      {/snippet}
    </Row>
  </Section>

  {#if reading}
    <Section title={sessionTime(reading.session.started_at)} testid="transcript-reader">
      <Row title={reading.session.source || "Saved session"} help={`${reading.lines.length} ${reading.lines.length === 1 ? "line" : "lines"}`}>
        {#snippet control()}<Button size="small" onclick={() => (reading = null)}>Back to the list</Button>{/snippet}
      </Row>
      <ol class="lines">
        {#each reading.lines as line, index (index)}
          <li class="line">
            <span class="time">{lineTime(line.wall)}</span>
            <div class="words">
              <span class="zh" lang="zh">{line.source}</span>
              {#if line.english}<span class="en">{line.english}</span>
              {:else if line.status === "skipped"}<Tag>Skipped to catch up</Tag>
              {:else if line.status === "failed"}<Tag tone="rose">Not translated</Tag>
              {:else if line.status === "english"}<Tag>English, not translated</Tag>{/if}
            </div>
          </li>
        {:else}
          <li class="empty">This session has no finished lines.</li>
        {/each}
      </ol>
    </Section>
  {:else}
    <Section title="Saved sessions" testid="transcripts">
      {#if sessions === null}
        <Row title="Looking for transcripts…" />
      {:else}
        {#each sessions as saved (saved.file)}
          <Row title={sessionTime(saved.started_at)} help={`${saved.source || "Saved session"} · ${saved.lines} ${saved.lines === 1 ? "line" : "lines"}`}>
            {#snippet control()}<Button size="small" onclick={() => openSession(saved)}>Read</Button>{/snippet}
          </Row>
        {:else}
          <Row title="No transcripts yet." help="They appear here after you listen with Save transcripts on." />
        {/each}
      {/if}
    </Section>
  {/if}
</section>

<style>
  .lines {
    margin: 0;
    padding: 0 18px 12px;
    list-style: none;
  }

  .line {
    display: grid;
    grid-template-columns: 70px minmax(0, 1fr);
    gap: 12px;
    padding: 10px 0;
    border-top: 1.5px dashed var(--line);
  }

  .time {
    color: var(--ink-3);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    padding-top: 2px;
  }

  .words {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-width: 0;
  }

  .zh {
    font-family: var(--font-cjk);
    color: var(--ink-2);
  }

  .en {
    font-weight: 700;
  }

  .empty {
    padding: 10px 0;
    color: var(--ink-2);
  }
</style>
