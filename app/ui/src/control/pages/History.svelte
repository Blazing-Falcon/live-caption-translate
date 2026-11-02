<script lang="ts">
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { transcriptPatch, type Config } from "../../lib/settings";
  import Button from "../../lib/ui/Button.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";
  import Switch from "../../lib/ui/Switch.svelte";

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
</section>
