<script lang="ts">
  import { untrack } from "svelte";
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
  let tops = new Map<string, { top: number; height: number }>();
  let primed = false;
  let pruneTimer: ReturnType<typeof setTimeout> | null = null;
  let resizeFrame = 0;

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

  function capOffset(): number {
    if (keep === null) return 0;
    const elements = blockElements();
    const extra = elements.length - keep;
    if (extra <= 0) return 0;
    return elements.slice(0, extra).reduce((sum, element) => sum + element.offsetHeight, 0);
  }

  function scrollToNeeded(): number {
    if (!viewport || !column) return 0;
    const overflow = Math.max(0, column.offsetHeight - viewport.clientHeight);
    return Math.max(snapUp(overflow), capOffset());
  }

  /** Move to `target`, animated unless `instant` or the reader asked for less motion. */
  function moveTo(target: number, instant: boolean): void {
    if (!column) return;
    if (instant || reducedMotion) {
      column.classList.add("jump");
      offset = target;
      return;
    }
    if (Math.abs(target - offset) < 0.5) return;
    column.classList.remove("jump");
    // Flush the class change so the transition is live when the transform moves.
    void column.offsetHeight;
    offset = target;
  }

  function measure(): void {
    if (!viewport || !column) return;
    const next = new Map<string, { top: number; height: number }>();
    for (const element of blockElements()) {
      next.set(element.dataset.block ?? "", { top: element.offsetTop, height: element.offsetHeight });
    }

    // Only a block that left the column above the top edge lowers the offset, by its height.
    let removed = 0;
    for (const [key, before] of tops) {
      if (next.has(key)) continue;
      if (before.top + before.height <= offset + 0.5) removed += before.height;
    }
    // A block above the top edge is clipped, so growing it is invisible, but it shifts every
    // visible block down by the same amount; raising the offset by that amount keeps them still.
    let above = 0;
    for (const [key, block] of next) {
      const before = tops.get(key);
      if (!before || before.top + before.height > offset + 0.5) continue;
      above = Math.max(above, block.top + block.height - (before.top + before.height));
    }
    tops = next;

    // The offset never falls, except by the height of blocks pruned above the top edge.
    const target = Math.max(
      Math.max(0, offset - removed) + Math.max(0, above),
      scrollToNeeded(),
    );
    // A prune's offset drop is instant: animating it would drag the visible blocks down with it.
    const instant = !primed || above > 0.5 || removed > 0.5;
    primed = true;
    moveTo(target, instant);
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

  // One observer for the component's life. Measuring inside the callback would re-enter the
  // observer and raise "ResizeObserver loop completed with undelivered notifications".
  $effect(() => {
    if (!viewport || !column) return;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(measure);
    });
    observer.observe(viewport);
    observer.observe(column);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(resizeFrame);
      if (pruneTimer !== null) clearTimeout(pruneTimer);
      pruneTimer = null;
    };
  });

  // Runs after the DOM for a new key set is in place, so the transition is live for that step.
  $effect(() => {
    void keys;
    untrack(measure);
  });
</script>

<div class="viewport" bind:this={viewport} style:height>
  <div
    class="column jump"
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
