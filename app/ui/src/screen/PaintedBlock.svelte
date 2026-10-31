<script lang="ts">
  import { tick, untrack } from "svelte";
  import { screenPlate } from "../lib/api";
  import { cssColor, toCssFloatRect, toCssRect, toCssPx } from "../lib/screen";
  import { ENGLISH_FONT, ENGLISH_FONT_WEIGHT, ENGLISH_LINE_SPACING, blockLayout, tiltTransform, type LayoutContext, type TextLayout } from "../lib/screen-layout";
  import { DomMeasurer } from "../lib/screen-measure";
  import type { ScreenBlock, ScreenFrame, ScreenPlate } from "../lib/types";

  let { frame, job, block, blocks, neighbours = blocks, context, text }: { frame: ScreenFrame; job: number; block: ScreenBlock; blocks: ScreenBlock[]; neighbours?: ScreenBlock[]; context: LayoutContext; text: string } = $props();
  let node = $state<HTMLElement>();
  let plate = $state<ScreenPlate | null>(null);
  let loaded = $state(false);
  let result = $state<TextLayout | null>(null);
  const css = $derived(result ? toCssFloatRect(result.bubble, frame.scale) : toCssRect(block.rect, frame.scale));
  const plateCss = $derived(plate ? toCssFloatRect(plate.rect, frame.scale) : null);
  const layoutKey = $derived(JSON.stringify([frame.id, frame.scale, job, block, blocks.map(b => b.paint.bounds), context, text,
    block.intent === "across_in_columns" ? neighbours.map(b => b.paint.bounds) : null]));
  const textCss = $derived(result ? toCssFloatRect(result.text, frame.scale) : null);
  const turn = $derived(result?.tilt ? tiltTransform(result.tilt) : undefined);
  const origin = (x: number, y: number): string | undefined => result?.tilt ?
    `${(result.tilt.centre.x - x) / frame.scale}px ${(result.tilt.centre.y - y) / frame.scale}px` : undefined;

  $effect(() => {
    if (!node) return;
    void layoutKey;
    const element = node;
    const current = untrack(() => ({ frame, job, block, blocks, neighbours, context, text }));
    let active = true;
    plate = null;
    loaded = false;
    result = null;
    const layout = async (): Promise<void> => {
      await document.fonts.ready;
      if (!active) return;
      const measurer = new DomMeasurer(current.frame.scale);
      let final: TextLayout;
      try { final = blockLayout(current.block, current.text, current.blocks, current.context, measurer, current.neighbours); }
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
    style:transform={plate.transform_with_block ? turn : undefined}
    style:transform-origin={plate.transform_with_block ? origin(plate.rect.x, plate.rect.y) : undefined}
    onload={() => { if (plate === shownPlate) loaded = true; }}
    onerror={() => { if (plate === shownPlate) { plate = null; loaded = false; } }}
  />
{/if}
<div
  bind:this={node}
  class="box"
  class:modern={!!result}
  class:positioned={!!result?.positioned}
  data-block={block.id}
  data-lines={result?.wrappedLineCount}
  data-source-font={result ? toCssPx(result.sourceFontSize, frame.scale) : undefined}
  style:left="{css.x}px"
  style:top="{css.y}px"
  style:width={result ? `${css.w}px` : undefined}
  style:height={result ? `${css.h}px` : undefined}
  style:font-size={result ? `${toCssPx(result.fontSize, frame.scale)}px` : undefined}
  style:line-height={result ? String(ENGLISH_LINE_SPACING) : undefined}
  style:visibility={!result ? "hidden" : "visible"}
  style:background={loaded ? "transparent" : cssColor(block.background)}
  style:color={cssColor(loaded && plate ? plate.foreground : block.foreground)}
  style:font-family={ENGLISH_FONT}
  style:font-weight={ENGLISH_FONT_WEIGHT}
  style:transform={turn}
  style:transform-origin={result ? origin(result.bubble.x, result.bubble.y) : undefined}
>{#if result}<span class="written" style:white-space={result.wrap ? "pre-wrap" : "pre"}
  style:left={result.positioned && textCss ? `${textCss.x - css.x}px` : undefined}
  style:top={result.positioned && textCss ? `${textCss.y - css.y}px` : undefined}
  style:width={result.positioned && textCss ? `${textCss.w}px` : undefined}
  style:text-align={result.textAlign}
>{result.content ?? text}</span>{:else}{text}{/if}</div>

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

  .positioned { display: block; padding: 0; }
  .positioned .written { position: absolute; }

  .plate {
    position: absolute;
    pointer-events: none;
    z-index: 0;
  }
</style>
