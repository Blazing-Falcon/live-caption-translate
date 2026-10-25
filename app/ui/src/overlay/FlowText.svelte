<script lang="ts">
  import { flowText, type Preview, type StatusLine } from "../lib/captions";
  import type { CaptionLine as Line } from "../lib/types";
  import CaptionLine from "./CaptionLine.svelte";

  interface Props {
    lines: readonly Line[];
    preview?: Preview | null;
    status?: StatusLine | null;
    showSource?: boolean;
    liveSource?: boolean;
    flowLines?: number;
  }

  let {
    lines,
    preview = null,
    status = null,
    showSource = true,
    liveSource = true,
    flowLines = 3,
  }: Props = $props();

  const flow = $derived(flowText(lines, showSource, liveSource, preview));
  const streaming = $derived(flow.english.at(-1)?.kind === "streaming");

  let section: HTMLElement | undefined = $state();
  let viewportHeight = $state(0);
  let fitted = $state<number | null>(null);
  const shownLines = $derived(Math.min(flowLines, fitted ?? flowLines));

  // A large font or many lines can make the box taller than the overlay window, which would
  // cut its top edge and a line in half. Show only the whole lines that fit.
  $effect(() => {
    void flow;
    void flowLines;
    void viewportHeight;
    const root = section?.closest<HTMLElement>(".overlay-root");
    if (!section || !root) return;
    let perLine = 0;
    let content = 0;
    for (const para of section.querySelectorAll<HTMLElement>(".para")) {
      perLine += parseFloat(getComputedStyle(para).lineHeight);
      content += para.offsetHeight;
    }
    if (perLine <= 0) return;
    const chrome = section.offsetHeight - content;
    const chip = root.querySelector<HTMLElement>(".chip-row")?.offsetHeight ?? 0;
    fitted = Math.max(1, Math.floor((root.clientHeight - chip - chrome) / perLine));
  });
</script>

<svelte:window bind:innerHeight={viewportHeight} />

<!-- Running text is pinned like live captions: the frame keeps its place and size through
     silence, so the reader's eyes never have to find it again. -->
<section
  bind:this={section}
  class="flow"
  aria-label="Live captions"
  data-testid="flow-text"
  style:--flow-lines={shownLines}
>
  {#if status}
    <CaptionLine {status} />
  {/if}
  {#if showSource || liveSource}
    <div class="para source">
      <p>
        {#each flow.source as piece (piece.id)}<span class:live={piece.kind === "live"} lang={piece.kind === "live" ? undefined : "zh"}>{piece.text}</span>{/each}
      </p>
    </div>
  {/if}
  <div class="para english">
    <p>
      {#if flow.english.length > 0}
        {#each flow.english as piece, index (`${piece.kind}-${piece.id}`)}{index > 0 ? " " : ""}<span class={piece.kind}>{piece.text}</span>{/each}{#if streaming}<span class="caret" aria-hidden="true"></span>{/if}
      {:else if !status && flow.source.length === 0}
        <span class="waiting">Waiting for speech</span>
      {/if}
    </p>
  </div>
</section>

<style>
  .flow {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    width: 100%;
    max-width: 960px;
    padding: var(--space-4) var(--space-6) 18px;
    background: var(--ov-bar-bg);
    border-radius: var(--radius-overlay);
  }

  /* Bottom-anchored and clipped to whole lines, so the newest text stays in view and
     older text leaves at the top. */
  .para {
    --flow-line-height: 1.35;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    height: calc(var(--flow-lines) * var(--flow-line-height) * 1em);
    overflow: hidden;
    line-height: var(--flow-line-height);
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .english {
    font-size: calc(var(--font-px, 26) * 1px);
    font-weight: 500;
    color: var(--ov-text);
    text-shadow: var(--ov-halo);
  }

  .source {
    font-family: var(--font-cjk);
    font-size: max(12px, calc(var(--font-px, 26) * 1px * var(--source-scale)));
    color: var(--ov-text-secondary);
    text-shadow: var(--ov-halo);
  }

  .pending {
    color: var(--ov-pending);
  }

  /* English that the next translation may still change. */
  .preview {
    color: var(--ov-text-secondary);
  }

  .waiting {
    color: var(--ov-pending);
  }

  /* Still being recognized: dimmer than settled Chinese, as in the panel. */
  .live {
    color: var(--ov-dim);
  }

  .caret {
    display: inline-block;
    width: 2px;
    height: 0.95em;
    margin-left: var(--space-1);
    vertical-align: -0.1em;
    background: var(--ov-accent);
    animation: caret-blink var(--caret-ms) steps(1) infinite;
  }

  @keyframes caret-blink {
    50% {
      opacity: 0;
    }
  }
</style>
