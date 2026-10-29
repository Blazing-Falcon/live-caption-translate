<script lang="ts">
  import { cssSize, rectBetween } from "../lib/screen";
  import type { Rect, ScreenFrame } from "../lib/types";

  let {
    frame,
    selection,
    onselect,
    onclose,
  }: {
    frame: ScreenFrame;
    /** The committed selection, CSS px. */
    selection: Rect | null;
    onselect: (rect: Rect) => void;
    onclose: () => void;
  } = $props();

  let origin = $state<{ x: number; y: number } | null>(null);
  let dragging = $state<Rect | null>(null);

  const shown = $derived(dragging ?? selection);

  function point(event: PointerEvent): { x: number; y: number } {
    const size = cssSize(frame);
    return {
      x: Math.min(Math.max(event.clientX, 0), size.w),
      y: Math.min(Math.max(event.clientY, 0), size.h),
    };
  }

  function down(event: PointerEvent): void {
    if (event.button === 2) {
      onclose();
      return;
    }
    if (event.button !== 0) return;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    origin = point(event);
    dragging = null;
  }

  function move(event: PointerEvent): void {
    if (origin) dragging = rectBetween(origin, point(event));
  }

  function up(event: PointerEvent): void {
    if (!origin) return;
    const rect = rectBetween(origin, point(event));
    origin = null;
    dragging = null;
    onselect(rect);
  }

  function cancel(): void {
    origin = null;
    dragging = null;
  }
</script>

<div
  class="selection"
  role="presentation"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={cancel}
>
  {#if shown}
    <div
      class="cutout"
      style:left="{shown.x}px"
      style:top="{shown.y}px"
      style:width="{shown.w}px"
      style:height="{shown.h}px"
    ></div>
  {:else}
    <div class="veil"></div>
  {/if}
</div>

<style>
  .selection {
    position: absolute;
    inset: 0;
    overflow: hidden;
    cursor: crosshair;
    touch-action: none;
  }

  .veil {
    position: absolute;
    inset: 0;
    background: var(--screen-dim);
    pointer-events: none;
  }

  /* The shadow dims everything outside the box. */
  .cutout {
    position: absolute;
    outline: var(--focus-width) dashed var(--ov-accent);
    box-shadow: 0 0 0 100vmax var(--screen-dim);
    pointer-events: none;
  }
</style>
