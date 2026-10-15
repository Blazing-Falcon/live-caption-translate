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
    panelLines?: number;
  }

  let {
    lines,
    fadingIds = new Set<number>(),
    status = null,
    showSource = true,
    liveSource = true,
    panelLines = 5,
  }: Props = $props();

  const shown = $derived(visibleLines(lines, "panel", panelLines, liveSource));
</script>

<section class="panel" aria-label="Live captions" data-testid="caption-panel">
  <div class="history">
    {#if status}
      <CaptionLine {status} />
    {:else}
      {#each shown as line, index (line.id)}
        <CaptionLine
          {line}
          {showSource}
          {liveSource}
          variant="panel"
          age={shown.length - 1 - index}
          fading={fadingIds.has(line.id)}
        />
      {/each}
    {/if}
  </div>
</section>

<style>
  .panel {
    position: relative;
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    overflow: hidden;
    background: var(--ov-bar-bg);
    border-radius: var(--radius-overlay);
  }

  .history {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: var(--panel-gap);
    padding: var(--space-4) 18px var(--space-5);
    overflow: hidden;
  }
</style>
