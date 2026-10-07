<script lang="ts">
  import { visibleLines, type StatusLine } from "../lib/captions";
  import type { CaptionLine as Line } from "../lib/types";
  import CaptionLine from "./CaptionLine.svelte";

  interface Props {
    lines: readonly Line[];
    fadingIds?: ReadonlySet<number>;
    status?: StatusLine | null;
    showSource?: boolean;
    panelLines?: number;
    headerText?: string;
    listening?: boolean;
    reveal?: boolean;
    onpause?: () => void;
    onsettings?: () => void;
  }

  let {
    lines,
    fadingIds = new Set<number>(),
    status = null,
    showSource = true,
    panelLines = 5,
    headerText = "",
    listening = true,
    reveal = false,
    onpause,
    onsettings,
  }: Props = $props();

  const shown = $derived(visibleLines(lines, "panel", panelLines));
</script>

<section class="panel" class:reveal aria-label="Live captions" data-testid="caption-panel">
  <header class="header" data-testid="panel-header">
    <div class="state">
      <span class="dot" class:hollow={!listening} aria-hidden="true"></span>
      <span>{headerText}</span>
    </div>
    <div class="actions">
      <button type="button" aria-label={listening ? "Pause" : "Resume"} onclick={() => onpause?.()}>
        {#if listening}
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true"><path d="M5.5 3.5v9M10.5 3.5v9" /></svg>
        {:else}
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" aria-hidden="true"><path d="M5 3.5l7 4.5-7 4.5z" /></svg>
        {/if}
      </button>
      <button type="button" aria-label="Open settings" onclick={() => onsettings?.()}>
        <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true"><path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11" /><circle class="knob" cx="6" cy="4.5" r="1.4" /><circle class="knob" cx="10.5" cy="8" r="1.4" /><circle class="knob" cx="5" cy="11.5" r="1.4" /></svg>
      </button>
    </div>
  </header>

  <div class="history">
    {#if status}
      <CaptionLine {status} />
    {:else}
      {#each shown as line, index (line.id)}
        <CaptionLine
          {line}
          {showSource}
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

  .header {
    position: absolute;
    inset: 0 0 auto 0;
    z-index: 1;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    padding: 10px var(--space-3) 10px var(--space-4);
    background: var(--ov-move-bg);
    border-bottom: 1px solid var(--ov-divider);
    visibility: hidden;
  }

  .panel:hover .header,
  .panel:focus-within .header,
  .panel.reveal .header {
    visibility: visible;
  }

  .state {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-help);
    color: var(--ov-chip-text);
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ov-accent);
  }

  .dot.hollow {
    background: transparent;
    border: 1.5px solid var(--ov-meta);
  }

  .actions {
    display: flex;
    gap: var(--space-1);
  }

  button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    padding: 0;
    border: 0;
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--ov-text-secondary);
    cursor: pointer;
  }

  button:hover {
    background: var(--ov-hover);
  }

  .knob {
    fill: var(--ov-move-bg);
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
