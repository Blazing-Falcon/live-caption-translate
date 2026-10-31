<script lang="ts">
  import { tick, untrack } from "svelte";
  import { screenPlate } from "../lib/api";
  import { cssColor, fitText, startFontPx, toCssFloatRect, toCssRect, toCssPx } from "../lib/screen";
  import { ENGLISH_FONT, ENGLISH_FONT_WEIGHT, ENGLISH_LINE_SPACING, horizontalLayout, type LayoutContext, type TextLayout } from "../lib/screen-layout";
  import { DomMeasurer } from "../lib/screen-measure";
  import type { ScreenBlock, ScreenFrame, ScreenPlate } from "../lib/types";

  let { frame, job, block, blocks, context, text }: { frame: ScreenFrame; job: number; block: ScreenBlock; blocks: ScreenBlock[]; context: LayoutContext; text: string } = $props();
  let node = $state<HTMLElement>();
  let plate = $state<ScreenPlate | null>(null);
  let loaded = $state(false);
  let result = $state<TextLayout | null>(null);
  const css = $derived(result ? toCssFloatRect(result.bubble, frame.scale) : toCssRect(block.rect, frame.scale));
  const plateCss = $derived(plate ? toCssFloatRect(plate.rect, frame.scale) : null);
  const layoutKey = $derived(JSON.stringify([frame.id, frame.scale, job, block, blocks.map(b => b.paint?.bounds ?? b.rect), context, text]));

  $effect(() => {
    if (!node) return;
    void layoutKey;
    const element = node;
    const current = untrack(() => ({ frame, job, block, blocks, context, text }));
    let active = true;
    plate = null;
    loaded = false;
    result = null;
    const layout = async (): Promise<void> => {
      await document.fonts.ready;
      if (!active) return;
      if (!current.block.paint) {
        const old = toCssRect(current.block.rect, current.frame.scale);
        fitText(element, old.w, old.h, startFontPx(current.block, current.frame.scale));
        return;
      }
      const measurer = new DomMeasurer(current.frame.scale);
      let final: TextLayout;
      try { final = horizontalLayout(current.block, current.text, current.blocks, current.context, measurer); }
      finally { measurer.dispose(); }
      result = final;
      await tick();
      if (!active) return;
      try {
        const image = await screenPlate({ frame: current.frame.id, job: current.job, block: current.block.id, rect: final.bubble, written_height: final.writtenHeight });
        if (active) plate = image;
      } catch {
        // The source colors remain usable while a newer job replaces this request.
      }
    };
    void layout();
    return () => { active = false; };
  });
</script>

{#if plate && plateCss}
  {@const shownPlate = plate}
  <img
    class="plate"
    data-plate={block.id}
    src={plate.url}
    alt=""
    style:left="{plateCss.x}px"
    style:top="{plateCss.y}px"
    style:width="{plateCss.w}px"
    style:height="{plateCss.h}px"
    onload={() => { if (plate === shownPlate) loaded = true; }}
    onerror={() => { if (plate === shownPlate) { plate = null; loaded = false; } }}
  />
{/if}
<div
  bind:this={node}
  class="box"
  class:modern={!!result}
  data-block={block.id}
  data-lines={result?.wrappedLineCount}
  data-source-font={result ? toCssPx(result.sourceFontSize, frame.scale) : undefined}
  style:left="{css.x}px"
  style:top="{css.y}px"
  style:width={result ? `${css.w}px` : undefined}
  style:height={result ? `${css.h}px` : undefined}
  style:font-size={result ? `${toCssPx(result.fontSize, frame.scale)}px` : undefined}
  style:line-height={result ? String(ENGLISH_LINE_SPACING) : undefined}
  style:visibility={block.paint && !result ? "hidden" : "visible"}
  style:background={loaded ? "transparent" : cssColor(block.background)}
  style:color={cssColor(loaded && plate ? plate.foreground : block.foreground)}
  style:font-family={ENGLISH_FONT}
  style:font-weight={ENGLISH_FONT_WEIGHT}
>{#if result}<span class="written" style:white-space={result.wrap ? "pre-wrap" : "pre"}>{text}</span>{:else}{text}{/if}</div>

<style>
  .box {
    position: absolute;
    overflow: hidden;
    padding: 1px;
    line-height: 1.15;
    z-index: 2;
  }

  .modern {
    display: flex;
    align-items: center;
    padding: 2px 3px;
    overflow: visible;
  }

  .written {
    width: 100%;
    flex-shrink: 0;
    overflow-wrap: anywhere;
    word-break: normal;
  }

  .plate {
    position: absolute;
    pointer-events: none;
    z-index: 0;
  }
</style>
