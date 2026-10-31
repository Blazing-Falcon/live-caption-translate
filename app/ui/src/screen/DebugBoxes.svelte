<script lang="ts">
  import { toCssFloatRect, toCssRect } from "../lib/screen";
  import { groupBoxes, lineBoxes } from "../lib/screen-debug";
  import type { Rect, ScreenBlock, ScreenFrame } from "../lib/types";

  let { frame, blocks, lines }: { frame: ScreenFrame; blocks: ScreenBlock[]; lines: Rect[] | null } = $props();
  const groups = $derived(groupBoxes(blocks));
  const raw = $derived(lines ?? lineBoxes(blocks));
</script>

<div class="debug" aria-hidden="true">
  {#each raw as line}
    {@const rect = toCssRect(line, frame.scale)}
    <div class="line" data-debug-line style:left="{rect.x}px" style:top="{rect.y}px" style:width="{rect.w}px" style:height="{rect.h}px"></div>
  {/each}
  {#each groups as group}
    {@const rect = toCssFloatRect(group, frame.scale)}
    <div class="group" data-debug-group style:left="{rect.x}px" style:top="{rect.y}px" style:width="{rect.w}px" style:height="{rect.h}px"></div>
  {/each}
</div>

<style>
  .debug { position: absolute; inset: 0; overflow: hidden; pointer-events: none; z-index: 3; }
  .line, .group { position: absolute; border: 1px solid; }
  .line { border-color: var(--screen-line-box); }
  .group { border-color: var(--screen-group-box); }
</style>
