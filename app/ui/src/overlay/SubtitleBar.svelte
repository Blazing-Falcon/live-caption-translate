<script lang="ts">
  import { visibleLines, type StatusLine } from "../lib/captions";
  import type { CaptionLine as Line } from "../lib/types";
  import CaptionLine from "./CaptionLine.svelte";

  interface Props {
    lines: readonly Line[];
    fadingIds?: ReadonlySet<number>;
    status?: StatusLine | null;
    showSource?: boolean;
    liveSource?: boolean;
  }

  let { lines, fadingIds = new Set<number>(), status = null, showSource = true, liveSource = true }: Props = $props();

  const shown = $derived(visibleLines(lines, "bar", 2, liveSource));
</script>

{#if status || shown.length > 0}
  <section class="bar" aria-label="Live captions" data-testid="subtitle-bar">
    {#if status}
      <CaptionLine {status} />
    {:else}
      {#each shown as line, index (line.id)}
        <CaptionLine
          {line}
          {showSource}
          {liveSource}
          variant="bar"
          age={shown.length - 1 - index}
          fading={fadingIds.has(line.id)}
        />
      {/each}
    {/if}
  </section>
{/if}

<style>
  .bar {
    display: flex;
    flex-direction: column;
    gap: var(--caption-gap);
    width: 100%;
    max-width: 960px;
    padding: var(--space-4) var(--space-6) 18px;
    background: var(--ov-bar-bg);
    border-radius: var(--radius-overlay);
  }
</style>
