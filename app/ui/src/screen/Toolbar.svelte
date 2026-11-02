<script lang="ts">
  import { onDestroy } from "svelte";
  import { modelsPercent } from "../lib/format";
  import { copyText, modelBusy, placeBar, statusText, type ScreenState, type Size } from "../lib/screen";
  import type { ScreenDirection } from "../lib/types";
  import Mascot from "../lib/ui/Mascot.svelte";

  type CopyKind = "english" | "chinese";

  let {
    view,
    hint,
    viewport,
    oncopy,
    ondownload,
    onoriginal,
    onclose,
    ondirection,
  }: {
    view: ScreenState;
    /** Why a block failed, while the pointer is on its mark; it replaces the status text. */
    hint: string | null;
    viewport: Size;
    oncopy: (kind: CopyKind) => Promise<boolean>;
    ondownload: () => void;
    onoriginal: (on: boolean) => void;
    onclose: () => void;
    ondirection: (direction: ScreenDirection) => void;
  } = $props();

  let width = $state(0);
  let height = $state(0);
  let copied = $state<{ kind: CopyKind; ok: boolean } | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;

  const COPIED_MS = 1500;

  const placement = $derived(view.selection ? placeBar(view.selection, { w: width, h: height }, viewport) : { left: Math.max(0, (viewport.w - width) / 2), top: 16 });
  const text = $derived(hint ?? (view.phase === "selecting" ? "Drag a box around Chinese text. Esc closes, D shows boxes." : statusText(view)));
  const mood = $derived(view.phase === "error" ? "trouble" : view.phase === "done" ? "listening" : "ready");
  const needsModel = $derived(view.model !== null && view.model.state !== "ready");
  const downloading = $derived(modelBusy(view.model));
  const hasEnglish = $derived(copyText(view, "english") !== "");
  const hasChinese = $derived(copyText(view, "chinese") !== "");

  async function copy(kind: CopyKind): Promise<void> {
    const ok = await oncopy(kind);
    copied = { kind, ok };
    clearTimeout(timer);
    timer = setTimeout(() => (copied = null), COPIED_MS);
  }

  function label(kind: CopyKind, idle: string): string {
    if (copied?.kind !== kind) return idle;
    return copied.ok ? "Copied" : "Copy failed";
  }

  function hold(event: PointerEvent): void {
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    onoriginal(true);
  }

  onDestroy(() => clearTimeout(timer));
</script>

{#if placement}
  <div
    class="bar"
    role="toolbar"
    aria-label="Screen translation"
    bind:offsetWidth={width}
    bind:offsetHeight={height}
    style:left="{placement.left}px"
    style:top="{placement.top}px"
    style:visibility={height === 0 ? "hidden" : "visible"}
  >
    <div class="status" role="status">
      <Mascot state={mood} size={22} />
      <span class="text" class:hint={hint !== null}>{text}</span>
      {#if downloading && view.model}
        <progress max="100" value={modelsPercent(view.model)} aria-label="Download progress"></progress>
      {/if}
    </div>
    {#if view.downloadError}
      <p class="warning">{view.downloadError}</p>
    {/if}
    <div class="actions">
      <label class="direction">Text direction
        <select aria-label="Text direction" value={view.direction} onchange={event => ondirection(event.currentTarget.value as ScreenDirection)}>
          <option value="horizontal">Across</option>
          <option value="vertical">Down</option>
        </select>
      </label>
      {#if needsModel && !downloading}
        <button type="button" onclick={ondownload}>Download</button>
      {/if}
      {#if view.selection && !needsModel && view.phase !== "error"}
        <button type="button" disabled={!hasEnglish} onclick={() => copy("english")}>{label("english", "Copy English")}</button>
        <button type="button" disabled={!hasChinese} onclick={() => copy("chinese")}>{label("chinese", "Copy Chinese")}</button>
        <button
          type="button"
          title="Hold to see the original. Holding Space does the same."
          aria-keyshortcuts="Space"
          onpointerdown={hold}
          onpointerup={() => onoriginal(false)}
          onpointercancel={() => onoriginal(false)}
          onlostpointercapture={() => onoriginal(false)}
          onkeydown={(event) => event.key === " " && event.preventDefault()}
        >
          Show original
        </button>
      {/if}
      <button type="button" onclick={onclose}>Close</button>
    </div>
  </div>
{/if}

<style>
  /* A sticky note: it keeps one look over any frozen screen, whatever the app theme. */
  .bar {
    --card: var(--screen-note-paper);
    --paper-2: var(--screen-note-paper);
    --ink: var(--screen-note-ink);
    --ink-3: var(--screen-note-ink);
    --pen-text: var(--screen-note-ink);
    --hl-rose: var(--screen-note-paper);
    --hl-mist: var(--screen-note-paper);
    --cheek: var(--screen-note-bg);
    z-index: 4;
    position: absolute;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: max-content;
    max-width: min(460px, calc(100vw - var(--space-4)));
    padding: 10px 12px;
    border: 2px solid var(--screen-note-ink);
    border-radius: 6px 6px 18px 6px;
    background: var(--screen-note-bg);
    box-shadow: var(--screen-note-shadow);
    color: var(--screen-note-ink);
    font: 700 13.5px / 1.35 var(--font-body);
    user-select: none;
  }

  .bar :focus-visible {
    outline: 2.5px solid var(--screen-note-ink);
    outline-offset: 2px;
  }

  .status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    min-height: var(--space-5);
  }

  .text.hint,
  .warning {
    color: var(--screen-note-warn);
  }

  .warning {
    margin: 0;
  }

  progress {
    width: 120px;
    accent-color: var(--screen-note-ink);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  button,
  select {
    min-height: 30px;
    padding: 0 12px;
    border: 2px solid var(--screen-note-ink);
    border-radius: 999px;
    background: var(--screen-note-paper);
    color: var(--screen-note-ink);
    font: inherit;
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    background: var(--screen-note-bg);
  }

  button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .direction {
    display: flex;
    align-items: center;
    gap: 6px;
  }
</style>
