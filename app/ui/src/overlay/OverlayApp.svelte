<script lang="ts">
  import * as api from "../lib/api";
  import type { Session } from "../lib/events";
  import { formatAccelerator } from "../lib/settings";
  import CaptionPanel from "./CaptionPanel.svelte";
  import FlowText from "./FlowText.svelte";
  import MoveFrame from "./MoveFrame.svelte";
  import StatusChip from "./StatusChip.svelte";
  import SubtitleBar from "./SubtitleBar.svelte";

  let { session }: { session: Session } = $props();

  // The session is fixed for the lifetime of the window.
  // svelte-ignore state_referenced_locally
  const { captions, config, appState } = session;

  const moving = $derived($appState?.overlay_moving ?? false);
  const lock = (): Promise<unknown> => session.act("lock", () => api.setOverlayMoving(false));
</script>

{#if $config}
  <main
    class="overlay-root"
    data-style={$config.overlay.style}
    style:--font-px={$config.overlay.font_px}
    style:--ov-bar-alpha={$config.overlay.background}
  >
    {#if $captions.chip}
      <div class="chip-row">
        <StatusChip chip={$captions.chip} />
      </div>
    {/if}
    {#if $config.overlay.style === "panel"}
      <CaptionPanel
        lines={$captions.lines}
        fadingIds={$captions.fadingIds}
        status={$captions.status}
        showSource={$config.overlay.show_source}
        liveSource={$config.overlay.live_source}
        panelLines={$config.overlay.panel_lines}
      />
    {:else if $config.overlay.style === "flow"}
      <div class="bar-slot">
        <FlowText
          lines={$captions.lines}
          status={$captions.status}
          showSource={$config.overlay.show_source}
          liveSource={$config.overlay.live_source}
          flowLines={$config.overlay.flow_lines}
        />
      </div>
    {:else}
      <div class="bar-slot">
        <SubtitleBar
          lines={$captions.lines}
          fadingIds={$captions.fadingIds}
          status={$captions.status}
          showSource={$config.overlay.show_source}
          liveSource={$config.overlay.live_source}
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

  .bar-slot {
    display: flex;
    justify-content: center;
  }

  [data-style="panel"] > :global(.panel) {
    flex: 1;
    min-height: 0;
  }
</style>
