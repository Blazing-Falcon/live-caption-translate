<script lang="ts">
  import { toCssRect, toFrameRect, type Outcome } from "../lib/screen";
  import PaintedBlock from "./PaintedBlock.svelte";
  import type { Rect, ScreenBlock, ScreenFrame } from "../lib/types";

  let {
    frame,
    job,
    selection,
    blocks,
    outcomes,
    hidden,
    onhint,
  }: {
    frame: ScreenFrame;
    job: number;
    selection: Rect | null;
    blocks: ScreenBlock[];
    outcomes: Record<number, Outcome>;
    /** Invisible but laid out, so a box that arrives meanwhile still fits. */
    hidden: boolean;
    /** The reason a block failed while the pointer is on its mark; null when it leaves. */
    onhint: (message: string | null) => void;
  } = $props();

  const context = $derived.by(() => {
    const r = selection ? toFrameRect(selection, frame) : { x: 0, y: 0, w: frame.width, h: frame.height };
    return { width: frame.width, height: frame.height, scale: frame.scale, selection: { x: r.x, y: r.y, width: r.w, height: r.h } };
  });

</script>

<div class="layer" style:visibility={hidden ? "hidden" : "visible"} data-testid="results">
  {#each blocks as block (block.id)}
    {@const outcome = outcomes[block.id]}
    {@const css = toCssRect(block.rect, frame.scale)}
    {#if outcome?.state === "translated"}
      <PaintedBlock {frame} {job} {block} {blocks} {context} text={outcome.text} />
    {:else if outcome?.state === "failed"}
      <div
        class="mark"
        data-failed={block.id}
        role="img"
        aria-label={outcome.message}
        title={outcome.message}
        onpointerenter={() => onhint(outcome.message)}
        onpointerleave={() => onhint(null)}
        style:left="{css.x}px"
        style:top="{css.y}px"
      >
        !
      </div>
    {/if}
  {/each}
</div>

<style>
  .layer {
    position: absolute;
    inset: 0;
    overflow: hidden;
    pointer-events: none;
  }

  .mark {
    z-index: 2;
    position: absolute;
    display: grid;
    place-items: center;
    width: var(--space-3);
    height: var(--space-3);
    border-radius: 50%;
    transform: translate(-50%, -50%);
    background: var(--ov-warning);
    color: var(--ov-button-text);
    font-size: var(--text-meta);
    font-weight: 700;
    line-height: 1;
    pointer-events: auto;
    cursor: help;
  }
</style>
