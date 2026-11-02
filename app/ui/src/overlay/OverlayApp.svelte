<script lang="ts">
  import * as api from "../lib/api";
  import type { Session } from "../lib/events";
  import { formatAccelerator } from "../lib/settings";
  import CaptionPanel from "./CaptionPanel.svelte";
  import FlowText from "./FlowText.svelte";
  import MoveFrame from "./MoveFrame.svelte";
  import StatusChip from "./StatusChip.svelte";

  let { session }: { session: Session } = $props();

  // The session is fixed for the lifetime of the window.
  // svelte-ignore state_referenced_locally
  const { captions, config, appState } = session;

  const moving = $derived($appState?.overlay_moving ?? false);
  const lock = (): Promise<unknown> => session.act("lock", () => api.setOverlayMoving(false));

  const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
  let reducedMotion = $state(motion.matches);
  $effect(() => {
    const update = (): void => {
      reducedMotion = motion.matches;
    };
    motion.addEventListener("change", update);
    return () => motion.removeEventListener("change", update);
  });
</script>

{#if $config}
  <main
    class="overlay-root"
    data-style={$config.overlay.style}
    data-card={$config.overlay.card}
    data-font={$config.overlay.font}
    style:--font-px={$config.overlay.font_px}
    style:--ov-bar-alpha={$config.overlay.background}
    style:--cap-en={$config.overlay.english_color}
    style:--cap-zh={$config.overlay.chinese_color}
  >
    {#if $captions.chip}
      <div class="chip-row">
        <StatusChip chip={$captions.chip} />
      </div>
    {/if}
    {#if $config.overlay.style === "panel"}
      <CaptionPanel
        blocks={$captions.blocks}
        status={$captions.status}
        showSource={$config.overlay.show_source}
        panelLines={$config.overlay.panel_lines}
        {reducedMotion}
      />
    {:else}
      <div class="flow-slot">
        <FlowText
          blocks={$captions.blocks}
          status={$captions.status}
          showSource={$config.overlay.show_source}
          flowLines={$config.overlay.flow_lines}
          {reducedMotion}
        />
      </div>
    {/if}
    {#if moving}
      <MoveFrame binding={formatAccelerator($config.hotkeys.move_lock)} onlock={lock} />
    {/if}
    <div class="sr-only" aria-live="polite" aria-atomic="true">{$captions.announcement}</div>
  </main>
{/if}

<style>
  .overlay-root {
    position: relative;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    height: 100%;
    min-height: 0;
  }

  .chip-row {
    display: flex;
    justify-content: flex-end;
    padding-bottom: var(--space-2);
  }

  .flow-slot {
    display: flex;
    justify-content: center;
  }

  [data-style="panel"] > :global(.panel) {
    flex: 1;
    min-height: 0;
  }
</style>
