<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { reconcileChecked, reconcileRadios } from "../../lib/reconcile";
  import {
    HOTKEY_ACTIONS,
    DRAFT_DISPLAYS,
    DRAFT_DISPLAY_HELP,
    LOW_BACKGROUND_WARNING,
    OVERLAY_LIMITS,
    hotkeyPatch,
    overlayPatch,
    type Config,
    type HotkeyAction,
  } from "../../lib/settings";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { appState, busy, hotkeyErrors, config: liveConfig } = session;

  const HOTKEY_LABELS: Record<HotkeyAction, string> = {
    move_lock: "Move or lock overlay",
    show_hide: "Show or hide overlay",
    pause: "Pause listening",
  };

  let fontDraft = $state<number | null>(null);
  let backgroundDraft = $state<number | null>(null);
  let expireDraft = $state<number | null>(null);
  let hotkeyDrafts = $state<Record<HotkeyAction, string>>({ move_lock: "", show_hide: "", pause: "" });
  let hotkeyBase = $state<Record<HotkeyAction, string>>({ move_lock: "", show_hide: "", pause: "" });

  const overlay = $derived(config.overlay);
  const fontPx = $derived(fontDraft ?? overlay.font_px);
  const backgroundPct = $derived(Math.round((backgroundDraft ?? overlay.background) * 100));
  const expireS = $derived(expireDraft ?? overlay.expire_s);
  const visible = $derived($appState?.overlay_visible ?? overlay.visible);
  const moving = $derived($appState?.overlay_moving ?? false);

  $effect(() => {
    for (const action of HOTKEY_ACTIONS) {
      const saved = config.hotkeys[action];
      if (hotkeyBase[action] !== saved) {
        hotkeyBase[action] = saved;
        hotkeyDrafts[action] = saved;
      }
    }
  });

  async function chooseStyle(event: Event, style: "bar" | "panel"): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("overlay.style", overlayPatch({ style }));
    reconcileRadios(input, get(liveConfig)?.overlay.style ?? overlay.style);
  }

  async function commitNumber(
    key: string,
    event: Event,
    field: "font_px" | "background" | "expire_s",
    scale: number,
    clear: () => void,
  ): Promise<void> {
    const value = Number((event.currentTarget as HTMLInputElement).value) / scale;
    await session.save(key, overlayPatch({ [field]: value }));
    clear();
  }

  async function toggleSource(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("overlay.show_source", overlayPatch({ show_source: input.checked }));
    reconcileChecked(input, get(liveConfig)?.overlay.show_source ?? overlay.show_source);
  }

  async function toggleLiveSource(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("overlay.live_source", overlayPatch({ live_source: input.checked }));
    reconcileChecked(input, get(liveConfig)?.overlay.live_source ?? overlay.live_source);
  }

  async function chooseDraftDisplay(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const chosen = DRAFT_DISPLAYS.find((option) => option.value === select.value);
    if (!chosen) {
      select.value = overlay.draft_display;
      return;
    }
    const saved = await session.save("overlay.draft_display", overlayPatch({ draft_display: chosen.value }));
    if (!saved) select.value = overlay.draft_display;
  }

  async function toggleVisible(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const done = await session.act("overlay.visible", () => api.setOverlayVisible(input.checked));
    if (done === null) reconcileChecked(input, visible);
  }

  function toggleMoving(): void {
    if (get(busy).has("overlay.moving")) return;
    void session.act("overlay.moving", () => api.setOverlayMoving(!moving));
  }

  async function chooseEdge(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("overlay.panel_edge", overlayPatch({ panel_edge: select.value === "left" ? "left" : "right" }));
    if (!saved) select.value = overlay.panel_edge;
  }

  async function chooseLines(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("overlay.panel_lines", overlayPatch({ panel_lines: Number(select.value) }));
    if (!saved) select.value = String(overlay.panel_lines);
  }

  async function saveHotkey(action: HotkeyAction): Promise<void> {
    if (get(busy).has(`hotkeys.${action}`)) return;
    const saved = await session.save(`hotkeys.${action}`, hotkeyPatch(action, hotkeyDrafts[action]));
    if (saved) session.clearHotkeyError(action);
    else hotkeyDrafts[action] = hotkeyBase[action];
  }
</script>

<section class="page" aria-labelledby="overlay-title">
  <h2 id="overlay-title" class="section-title">Overlay</h2>

  <div class="field" role="radiogroup" aria-label="Style">
    <span>Style</span>
    <div class="segmented">
      <label>
        <input type="radio" name="style" value="bar" checked={overlay.style === "bar"} onchange={(event) => chooseStyle(event, "bar")} />
        <span>Subtitle bar</span>
      </label>
      <label>
        <input type="radio" name="style" value="panel" checked={overlay.style === "panel"} onchange={(event) => chooseStyle(event, "panel")} />
        <span>Caption panel</span>
      </label>
    </div>
  </div>

  <label class="field">
    <span>English text size · {fontPx} px</span>
    <input
      class="range"
      type="range"
      min={OVERLAY_LIMITS.font_px.min}
      max={OVERLAY_LIMITS.font_px.max}
      step="1"
      value={fontPx}
      oninput={(event) => (fontDraft = Number(event.currentTarget.value))}
      onchange={(event) => commitNumber("overlay.font_px", event, "font_px", 1, () => (fontDraft = null))}
    />
  </label>

  <div class="field">
    <label for="overlay-background">Background · {backgroundPct}%</label>
    <input
      id="overlay-background"
      class="range"
      type="range"
      min="0"
      max="100"
      step="1"
      value={backgroundPct}
      oninput={(event) => (backgroundDraft = Number(event.currentTarget.value) / 100)}
      onchange={(event) => commitNumber("overlay.background", event, "background", 100, () => (backgroundDraft = null))}
    />
    {#if backgroundPct / 100 < LOW_BACKGROUND_WARNING}
      <span class="help small warning" role="note" data-testid="background-warning">May be hard to read on bright video</span>
    {/if}
  </div>

  <label class="check-row">
    <input type="checkbox" checked={overlay.show_source} onchange={toggleSource} />
    <span>Show the Chinese line above the English</span>
  </label>

  <label class="check-row">
    <input type="checkbox" checked={overlay.live_source} onchange={toggleLiveSource} />
    <span>Show Chinese while someone is speaking</span>
  </label>

  <div class="stack">
    <label class="field">
      <span>Draft text</span>
      <select class="select" value={overlay.draft_display} aria-describedby="draft-display-help" onchange={chooseDraftDisplay}>
        {#each DRAFT_DISPLAYS as option (option.value)}
          <option value={option.value}>{option.label}</option>
        {/each}
      </select>
    </label>
    <p id="draft-display-help" class="help small">{DRAFT_DISPLAY_HELP}</p>
  </div>

  <label class="field">
    <span>Clear captions after {expireS} s without speech</span>
    <input
      class="range"
      type="range"
      min={OVERLAY_LIMITS.expire_s.min}
      max={OVERLAY_LIMITS.expire_s.max}
      step="1"
      value={expireS}
      oninput={(event) => (expireDraft = Number(event.currentTarget.value))}
      onchange={(event) => commitNumber("overlay.expire_s", event, "expire_s", 1, () => (expireDraft = null))}
    />
  </label>

  {#if overlay.style === "panel"}
    <div class="stack">
      <label class="field">
        <span>Caption panel edge</span>
        <select class="select" value={overlay.panel_edge} onchange={chooseEdge}>
          <option value="right">Right</option>
          <option value="left">Left</option>
        </select>
      </label>
      <label class="field">
        <span>Lines kept in the panel</span>
        <select class="select" value={String(overlay.panel_lines)} onchange={chooseLines}>
          {#each [4, 5, 6] as count (count)}
            <option value={String(count)}>{count}</option>
          {/each}
        </select>
      </label>
    </div>
  {/if}

  <div class="stack">
    <label class="check-row">
      <input type="checkbox" checked={visible} onchange={toggleVisible} />
      <span>Show the overlay</span>
    </label>
    <div>
      <button type="button" class="btn" aria-pressed={moving} aria-disabled={$busy.has("overlay.moving")} onclick={toggleMoving}>
        {moving ? "Lock overlay" : "Move overlay"}
      </button>
    </div>
  </div>

  <div class="stack" role="group" aria-label="Keyboard shortcuts">
    <h3 class="section-title">Shortcuts</h3>
    {#each HOTKEY_ACTIONS as action (action)}
      {@const failure = $hotkeyErrors[action]}
      <div class="hotkey-row">
        <label for={`hotkey-${action}`}>{HOTKEY_LABELS[action]}</label>
        <input
          id={`hotkey-${action}`}
          class="input mono"
          type="text"
          spellcheck="false"
          autocomplete="off"
          placeholder="Off"
          bind:value={hotkeyDrafts[action]}
          aria-invalid={failure ? "true" : undefined}
          aria-describedby={failure ? `hotkey-error-${action}` : undefined}
          onkeydown={(event) => event.key === "Enter" && saveHotkey(action)}
        />
        <button
          type="button"
          class="btn"
          disabled={hotkeyDrafts[action].trim() === hotkeyBase[action]}
          aria-disabled={$busy.has(`hotkeys.${action}`)}
          aria-label={`Save ${HOTKEY_LABELS[action]} shortcut`}
          onclick={() => saveHotkey(action)}
        >
          Save
        </button>
        {#if failure}
          <p id={`hotkey-error-${action}`} class="error warning" role="alert">
            {HOTKEY_LABELS[action]}: {failure.accelerator || "this shortcut"} could not be used. {failure.message}
          </p>
        {/if}
      </div>
    {/each}
    <p class="help small">Leave a shortcut empty to turn it off. Example: Ctrl+Shift+L</p>
  </div>
</section>
