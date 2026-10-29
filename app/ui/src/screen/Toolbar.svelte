<script lang="ts">
  import { onDestroy } from "svelte";
  import { modelsPercent } from "../lib/format";
  import { copyText, modelBusy, placeBar, statusText, type ScreenState, type Size } from "../lib/screen";

  type CopyKind = "english" | "chinese";

  let {
    view,
    hint,
    viewport,
    oncopy,
    ondownload,
    onoriginal,
    onclose,
  }: {
    view: ScreenState;
    /** Why a block failed, while the pointer is on its mark; it replaces the status text. */
    hint: string | null;
    viewport: Size;
    oncopy: (kind: CopyKind) => Promise<boolean>;
    ondownload: () => void;
    onoriginal: (on: boolean) => void;
    onclose: () => void;
  } = $props();

  let width = $state(0);
  let height = $state(0);
  let copied = $state<{ kind: CopyKind; ok: boolean } | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;

  const COPIED_MS = 1500;

  const placement = $derived(view.selection ? placeBar(view.selection, { w: width, h: height }, viewport) : null);
  const text = $derived(hint ?? statusText(view));
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
      <span class="text" class:hint={hint !== null}>{text}</span>
      {#if downloading && view.model}
        <progress max="100" value={modelsPercent(view.model)} aria-label="Download progress"></progress>
      {/if}
    </div>
    {#if view.downloadError}
      <p class="warning">{view.downloadError}</p>
    {/if}
    <div class="actions">
      {#if needsModel && !downloading}
        <button type="button" onclick={ondownload}>Download</button>
      {/if}
      {#if !needsModel && view.phase !== "error"}
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
  .bar {
    position: absolute;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    width: max-content;
    max-width: min(440px, calc(100vw - var(--space-4)));
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--ov-divider);
    border-radius: var(--radius-card);
    background: var(--screen-bar-bg);
    color: var(--ov-text);
    font: var(--text-label) var(--font-ui);
    user-select: none;
  }

  .bar :focus-visible {
    outline-color: var(--ov-accent);
  }

  .status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--space-5);
  }

  .text {
    color: var(--ov-text-bright);
  }

  .text.hint {
    color: var(--ov-warning);
  }

  .warning {
    margin: 0;
    color: var(--ov-warning);
  }

  progress {
    width: 120px;
    accent-color: var(--ov-accent);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  button {
    min-height: calc(var(--control-h) - var(--space-2));
    padding: 0 var(--space-3);
    border: 1px solid var(--ov-key-border);
    border-radius: var(--radius-control);
    background: transparent;
    color: var(--ov-text);
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    background: var(--ov-hover);
  }

  button:disabled {
    color: var(--ov-meta);
    cursor: default;
  }
</style>
