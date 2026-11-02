<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../lib/api";
  import type { Session } from "../lib/events";
  import { OVERLAY_LIMITS, overlayPatch, type Config } from "../lib/settings";
  import Button from "../lib/ui/Button.svelte";
  import Keys from "../lib/ui/Keys.svelte";
  import Mascot from "../lib/ui/Mascot.svelte";
  import Segmented from "../lib/ui/Segmented.svelte";
  import Slider from "../lib/ui/Slider.svelte";
  import Switch from "../lib/ui/Switch.svelte";
  import { homeStatus, translatorLabel } from "../lib/wording";

  /** The switches people change during a game or a show; everything else is one click away. */
  let { session }: { session: Session } = $props();

  // The session is fixed for the lifetime of the window.
  // svelte-ignore state_referenced_locally
  const { config, appState, busy } = session;

  let fontDraft = $state<number | null>(null);

  const listening = $derived($appState?.listening ?? "starting");
  const visible = $derived($appState?.overlay_visible ?? $config?.overlay.visible ?? true);
  const moving = $derived($appState?.overlay_moving ?? false);

  const save = (key: string, values: Partial<Config["overlay"]>) => session.save(`overlay.${key}`, overlayPatch(values));

  async function toggleRun(): Promise<void> {
    if (get(busy).has("run") || listening === "starting") return;
    await session.act("run", () => (listening === "listening" ? api.pauseListening() : api.startListening()));
  }
</script>

{#if $config}
  {@const status = homeStatus($appState, $config)}
  <main class="tray" aria-label="Quick settings">
    <header data-testid="tray-status">
      <Mascot state={status.mood} size={40} />
      <div class="words">
        <b>{status.mood === "listening" ? "Listening" : status.title}</b>
        <span>{translatorLabel($config.translate.engine)}</span>
      </div>
      <Button variant={listening === "paused" ? "primary" : "normal"} size="small" busy={$busy.has("run") || listening === "starting"} onclick={toggleRun}>
        {listening === "listening" ? "Pause" : listening === "paused" ? "Start" : "Starting…"}
      </Button>
    </header>

    <div class="rows">
      <div class="row">
        <span>Captions</span>
        <Switch label="Captions" checked={visible} onchange={(on) => session.act("overlay.visible", () => api.setOverlayVisible(on))} />
      </div>
      <div class="row">
        <span>Chinese line</span>
        <Switch label="Chinese line" checked={$config.overlay.show_source} onchange={(show_source) => save("show_source", { show_source })} />
      </div>
      <div class="row stacked">
        <span>Layout</span>
        <Segmented
          name="tray-style"
          label="Layout"
          value={$config.overlay.style}
          options={[
            { value: "panel", label: "Panel" },
            { value: "flow", label: "Running" },
            { value: "subtitles", label: "Subtitles" },
          ]}
          onchange={(style) => save("style", { style })}
        />
      </div>
      <div class="row stacked">
        <span>Size</span>
        <Slider
          id="tray-size"
          label="English size"
          min={OVERLAY_LIMITS.font_px.min}
          max={OVERLAY_LIMITS.font_px.max}
          value={fontDraft ?? $config.overlay.font_px}
          format={(value) => `${value} px`}
          oninput={(value) => (fontDraft = value)}
          onchange={async (font_px) => {
            await save("font_px", { font_px });
            fontDraft = null;
          }}
        />
      </div>
      {#if $config.hotkeys.screen_translate}
        <div class="row"><span>Translate screen</span><Keys combo={$config.hotkeys.screen_translate} /></div>
      {/if}
    </div>

    <footer>
      <Button variant="quiet" size="small" busy={$busy.has("overlay.moving")} onclick={() => session.act("overlay.moving", () => api.setOverlayMoving(!moving))}>
        {moving ? "Lock captions" : "Move captions"}
      </Button>
      <Button variant="quiet" size="small" onclick={() => session.act("open.settings", () => api.openSettings())}>All settings →</Button>
    </footer>
  </main>
{/if}

<style>
  :global(body.tray-body) {
    overflow: hidden;
  }

  .tray {
    display: flex;
    flex-direction: column;
    height: 100vh;
    border: var(--stroke-w) solid var(--stroke);
    background: var(--card);
  }

  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 16px;
    border-bottom: 1.5px dashed var(--line);
  }

  .words {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .words b {
    font: 600 18px / 1.2 var(--font-head);
  }

  .words span {
    color: var(--ink-2);
    font-size: 13px;
  }

  .rows {
    flex: 1;
    display: flex;
    flex-direction: column;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 16px;
    border-top: 1.5px dashed var(--line);
    font-weight: 700;
  }

  .row:first-child {
    border-top: 0;
  }

  .stacked {
    flex-direction: column;
    align-items: stretch;
    gap: 6px;
  }

  footer {
    display: flex;
    justify-content: space-between;
    padding: 8px 12px;
    border-top: 1.5px dashed var(--line);
    background: var(--paper-2);
  }
</style>
