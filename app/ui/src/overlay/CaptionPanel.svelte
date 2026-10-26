<script lang="ts">
  import type { CaptionBlock, StatusLine } from "../lib/captions";
  import { OVERLAY_LIMITS } from "../lib/settings";
  import CaptionBlockView from "./CaptionBlock.svelte";
  import CaptionViewport from "./CaptionViewport.svelte";
  import StatusPill from "./StatusPill.svelte";

  interface Props {
    blocks: readonly CaptionBlock[];
    status?: StatusLine | null;
    showSource?: boolean;
    panelLines?: number;
    reducedMotion?: boolean;
  }

  let {
    blocks,
    status = null,
    showSource = true,
    panelLines = 5,
    reducedMotion = false,
  }: Props = $props();

  const keep = $derived(
    Math.min(OVERLAY_LIMITS.panel_lines.max, Math.max(OVERLAY_LIMITS.panel_lines.min, Math.round(panelLines))),
  );
  let pruned = $state<ReadonlySet<number>>(new Set());
  const shown = $derived(blocks.filter((block) => !pruned.has(block.key)));
  const keys = $derived(shown.map((block) => block.key));

  function prune(gone: number[]): void {
    pruned = new Set([...pruned, ...gone]);
  }

  $effect(() => {
    const live = new Set(blocks.map((block) => block.key));
    if ([...pruned].some((key) => !live.has(key))) {
      pruned = new Set([...pruned].filter((key) => live.has(key)));
    }
  });
</script>

<section class="panel" aria-label="Live captions" data-testid="caption-panel">
  <StatusPill {status} />
  {#if shown.length === 0 && !status}
    <p class="waiting">Waiting for speech</p>
  {/if}
  <CaptionViewport
    snap="block"
    {keys}
    keep={keep}
    {reducedMotion}
    onprune={prune}
    gap="0"
    padding="0 18px calc(var(--space-5) - var(--panel-gap))"
  >
    {#each shown as block, index (block.key)}
      <CaptionBlockView
        {block}
        {showSource}
        age={shown.length - 1 - index}
      />
    {/each}
  </CaptionViewport>
</section>

<style>
  .panel {
    position: relative;
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    padding-top: calc(var(--space-4) + var(--status-h) + var(--space-2));
    overflow: hidden;
    background: var(--ov-bar-bg);
    border-radius: var(--radius-overlay);
  }

  .waiting {
    position: absolute;
    left: 18px;
    bottom: var(--space-5);
    margin: 0;
    font-size: calc(var(--font-px, 26) * 1px);
    font-weight: 500;
    color: var(--ov-pending);
    text-shadow: var(--ov-halo);
  }
</style>
