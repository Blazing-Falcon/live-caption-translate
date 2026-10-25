<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    /** `block` rounds the scroll up to a block's top edge; `line` to a whole line. */
    snap: "block" | "line";
    /** The block keys in the column, in order: a change means the column must be measured. */
    keys?: readonly number[];
    /** The fixed CSS height of the box. */
    height?: string;
    reducedMotion?: boolean;
    /** Blocks to keep once they are fully above the viewport; null keeps every block. */
    keep?: number | null;
    onprune?: (keys: number[]) => void;
    gap?: string;
    padding?: string;
    children: Snippet;
  }

  let {
    snap,
    keys = [],
    height = "100%",
    reducedMotion = false,
    keep = null,
    onprune,
    gap = "0",
    padding = "0",
    children,
  }: Props = $props();

  const SCROLL_MS = 180;
  const PRUNE_DELAY_MS = SCROLL_MS + 40;

  let viewport: HTMLElement | undefined = $state();
  let column: HTMLElement | undefined = $state();
  let offset = $state(0);
  let jump = $state(true);
  let tops = new Map<string, number>();
  let primed = false;
  let pruneTimer: ReturnType<typeof setTimeout> | null = null;

  function blockElements(): HTMLElement[] {
    return column ? [...column.querySelectorAll<HTMLElement>("[data-block]")] : [];
  }

  function lineHeight(): number {
    const first = column?.firstElementChild;
    return first ? parseFloat(getComputedStyle(first).lineHeight) || 0 : 0;
  }

  /** The first block top (or whole line) at or below `value`. */
  function snapUp(value: number): number {
    if (value <= 0) return 0;
    if (snap === "line") {
      const line = lineHeight();
      return line > 0 ? Math.ceil(value / line - 0.01) * line : value;
    }
    for (const element of blockElements()) {
      if (element.offsetTop >= value - 0.5) return element.offsetTop;
    }
    return value;
  }

  function scrollToNeeded(): number {
    if (!viewport || !column) return 0;
    return snapUp(Math.max(0, column.offsetHeight - viewport.clientHeight));
  }

  function measure(): void {
    if (!viewport || !column) return;
    const next = new Map<string, number>();
    for (const element of blockElements()) next.set(element.dataset.block ?? "", element.offsetTop);

    let removed = 0;
    let down = 0;
    for (const [key, top] of next) {
      const before = tops.get(key);
      if (before === undefined) continue;
      removed = Math.max(removed, before - top);
      down = Math.max(down, top - before);
    }
    tops = next;

    const base = Math.max(0, offset - removed);
    // A block that grew pushes the ones below it down; scrolling by the same amount keeps every
    // block that is already visible where it is, and the offset only ever grows.
    const target = Math.max(base, snapUp(base + down), scrollToNeeded());
    if (!primed) {
      primed = true;
      jump = true;
      offset = target;
      jump = false;
      return;
    }
    if (target > offset + 0.5) {
      // Content above a visible block grew, so the offset grows by the same amount at once and
      // the block stays put; a plain roll past the bottom is the one animated step.
      jump = reducedMotion || down > 0.5;
      offset = target;
    } else {
      // Pruned blocks leave no visible change: their height comes out of the offset in the same
      // frame, so nothing the reader sees moves.
      jump = true;
      offset = target;
    }
    schedulePrune();
  }

  function schedulePrune(): void {
    if (keep === null || !onprune) return;
    if (pruneTimer !== null) clearTimeout(pruneTimer);
    pruneTimer = setTimeout(prune, PRUNE_DELAY_MS);
  }

  function prune(): void {
    pruneTimer = null;
    if (keep === null || !onprune) return;
    const elements = blockElements();
    if (elements.length <= keep) return;
    const keys: number[] = [];
    for (const element of elements.slice(0, elements.length - keep)) {
      if (element.offsetTop + element.offsetHeight > offset + 0.5) break;
      keys.push(Number(element.dataset.block));
    }
    if (keys.length > 0) onprune(keys);
  }

  $effect(() => {
    void keys;
    if (!viewport || !column) return;
    const observer = new ResizeObserver(measure);
    observer.observe(viewport);
    observer.observe(column);
    measure();
    return () => {
      observer.disconnect();
      if (pruneTimer !== null) clearTimeout(pruneTimer);
      pruneTimer = null;
    };
  });
</script>

<div class="viewport" bind:this={viewport} style:height>
  <div
    class="column"
    class:jump
    bind:this={column}
    style:transform="translateY({-offset}px)"
    style:gap
    style:padding
  >
    {@render children()}
  </div>
</div>

<style>
  .viewport {
    position: relative;
    width: 100%;
    min-height: 0;
    overflow: hidden;
  }

  .column {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    transition: transform 180ms ease-out;
  }

  .column.jump {
    transition: none;
  }
</style>
