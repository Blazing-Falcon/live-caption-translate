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

  // The status is a separate flex item above the history, so the history gives up its oldest
  // line rather than the status; the count is the user's own setting either way.
  const shown = $derived(visibleLines(lines, "panel", panelLines, liveSource));
</script>

<section class="panel" aria-label="Live captions" data-testid="caption-panel">
  {#if status}
    <div class="status-row"><CaptionLine {status} /></div>
  {/if}
  <div class="history">
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

  /* The status is its own flex item, so a full history shrinks the captions instead of pushing
   * the status off the top: the history is bottom-anchored and clips its oldest line, while the
   * status keeps its height. */
  .status-row {
    flex: none;
    padding: var(--space-4) 18px 0;
  }

  .history {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: var(--panel-gap);
    padding: var(--space-4) 18px var(--space-5);
    overflow: hidden;
  }
</style>
