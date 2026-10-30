<script lang="ts">
  import { untrack } from "svelte";
  import { screenPlate } from "../lib/api";
  import { cssColor, fitText, startFontPx, toCssFloatRect, toCssRect, toFrameFloatRect, toFramePx } from "../lib/screen";
  import { ENGLISH_FONT, ENGLISH_FONT_WEIGHT } from "../lib/screen-layout";
  import type { ScreenBlock, ScreenFrame, ScreenPlate } from "../lib/types";

  let { frame, job, block, text }: { frame: ScreenFrame; job: number; block: ScreenBlock; text: string } = $props();
  let node = $state<HTMLElement>();
  let plate = $state<ScreenPlate | null>(null);
  let loaded = $state(false);
  const css = $derived(toCssRect(block.rect, frame.scale));
  const plateCss = $derived(plate ? toCssFloatRect(plate.rect, frame.scale) : null);
  const layoutKey = $derived(JSON.stringify([frame.id, frame.scale, job, block.id, block.rect, block.line_height, !!block.paint, text]));

  $effect(() => {
    if (!node) return;
    void layoutKey;
    const element = node;
    const current = untrack(() => ({ frame, job, block, text, css }));
    let active = true;
    plate = null;
    loaded = false;
    const layout = async (): Promise<void> => {
      await document.fonts.ready;
      if (!active) return;
      fitText(element, current.css.w, current.css.h, startFontPx(current.block, current.frame.scale));
      if (!current.block.paint) return;
      const range = document.createRange();
      range.selectNodeContents(element);
      const written = range.getBoundingClientRect().height;
      const rect = toFrameFloatRect({ x: current.css.x, y: current.css.y, w: element.offsetWidth, h: element.offsetHeight }, current.frame.scale);
      try {
        const result = await screenPlate({ frame: current.frame.id, job: current.job, block: current.block.id, rect, written_height: toFramePx(written, current.frame.scale) });
        if (active) plate = result;
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
  data-block={block.id}
  style:left="{css.x}px"
  style:top="{css.y}px"
  style:background={loaded ? "transparent" : cssColor(block.background)}
  style:color={cssColor(loaded && plate ? plate.foreground : block.foreground)}
  style:font-family={ENGLISH_FONT}
  style:font-weight={ENGLISH_FONT_WEIGHT}
>{text}</div>

<style>
  .box {
    position: absolute;
    overflow: hidden;
    padding: 1px;
    line-height: 1.15;
  }

  .plate {
    position: absolute;
    pointer-events: none;
  }
</style>
