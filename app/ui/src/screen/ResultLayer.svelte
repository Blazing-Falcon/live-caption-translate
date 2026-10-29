<script lang="ts">
  import { cssColor, fitText, startFontPx, toCssRect, type Outcome } from "../lib/screen";
  import type { ScreenBlock, ScreenFrame } from "../lib/types";

  let {
    frame,
    blocks,
    outcomes,
    hidden,
    onhint,
  }: {
    frame: ScreenFrame;
    blocks: ScreenBlock[];
    outcomes: Record<number, Outcome>;
    /** Invisible but laid out, so a box that arrives meanwhile still fits. */
    hidden: boolean;
    /** The reason a block failed while the pointer is on its mark; null when it leaves. */
    onhint: (message: string | null) => void;
  } = $props();

  function fit(node: HTMLElement, params: { width: number; height: number; start: number }): void {
    fitText(node, params.width, params.height, params.start);
  }
</script>

<div class="layer" style:visibility={hidden ? "hidden" : "visible"} data-testid="results">
  {#each blocks as block (block.id)}
    {@const outcome = outcomes[block.id]}
    {@const css = toCssRect(block.rect, frame.scale)}
    {#if outcome?.state === "translated"}
      <div
        class="box"
        data-block={block.id}
        style:left="{css.x}px"
        style:top="{css.y}px"
        style:--block-bg={cssColor(block.background)}
        style:--block-fg={cssColor(block.foreground)}
        use:fit={{ width: css.w, height: css.h, start: startFontPx(block, frame.scale) }}
      >
        {outcome.text}
      </div>
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

  .box {
    position: absolute;
    overflow: hidden;
    padding: 1px;
    background: var(--block-bg);
    color: var(--block-fg);
    font-family: var(--font-ui);
    line-height: 1.15;
  }

  .mark {
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
