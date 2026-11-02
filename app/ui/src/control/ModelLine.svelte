<script lang="ts">
  import { formatBytes, modelsPercent } from "../lib/format";
  import type { ModelStatus } from "../lib/types";
  import Progress from "../lib/ui/Progress.svelte";
  import Tag from "../lib/ui/Tag.svelte";

  /** A model's state in words, with a bar while it downloads. */
  let { model, label }: { model: ModelStatus | undefined; label: string } = $props();

  const percent = $derived(model ? modelsPercent(model) : 0);
</script>

{#if model}
  <div class="line" data-testid={`model-${model.id}`} data-state={model.state}>
    {#if model.state === "ready"}
      <Tag tone="sage">Ready</Tag>
    {:else if model.state === "downloading"}
      <span class="progress"><Progress {percent} label={`${label} download`} /></span>
      <span class="words">{formatBytes(model.bytes_done)} of {formatBytes(model.bytes_total)}</span>
    {:else if model.state === "verifying"}
      <Tag tone="lemon">Checking the file…</Tag>
    {:else if model.state === "paused"}
      <Tag tone="lemon">Paused · {percent}%</Tag>
    {:else if model.state === "corrupt"}
      <Tag tone="rose">Damaged</Tag>
    {:else}
      <Tag>Not downloaded · {formatBytes(model.bytes_total)}</Tag>
    {/if}
  </div>
{/if}

<style>
  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }

  .progress {
    width: 160px;
  }

  .words {
    color: var(--ink-2);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }
</style>
