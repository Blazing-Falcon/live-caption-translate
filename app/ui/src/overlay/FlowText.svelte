<script lang="ts">
  import { flowPieces, type CaptionBlock, type StatusLine } from "../lib/captions";
  import CaptionViewport from "./CaptionViewport.svelte";
  import StatusPill from "./StatusPill.svelte";

  interface Props {
    blocks: readonly CaptionBlock[];
    status?: StatusLine | null;
    showSource?: boolean;
    flowLines?: number;
    reducedMotion?: boolean;
  }

  let {
    blocks,
    status = null,
    showSource = true,
    flowLines = 3,
    reducedMotion = false,
  }: Props = $props();

  const flow = $derived(flowPieces(blocks, showSource));
  const streaming = $derived(flow.english.at(-1)?.kind === "streaming");
  const showSourceParagraph = $derived(showSource || blocks.some((block) => block.live !== ""));

  let section: HTMLElement | undefined = $state();
  let windowHeight = $state(0);
  let fitted = $state<number | null>(null);
  const lines = $derived(Math.min(flowLines, fitted ?? flowLines));

  // A large font or many lines can make the box taller than the overlay window, which would
  // cut its top edge and a line in half. Show only the whole lines that fit.
  $effect(() => {
    void flowLines;
    void windowHeight;
    void flow.source;
    void flow.english;
    void showSourceParagraph;
    const root = section?.closest<HTMLElement>(".overlay-root");
    if (!section || !root) return;
    const style = getComputedStyle(section);
    const paragraphs = [...section.querySelectorAll<HTMLElement>(".para")];
    if (paragraphs.length === 0) return;
    const perLine = paragraphs.reduce((sum, para) => sum + parseFloat(getComputedStyle(para).lineHeight), 0);
    if (perLine <= 0) return;
    const gap = parseFloat(style.rowGap) || 0;
    const chrome = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom) + gap * (paragraphs.length - 1);
    const chip = root.querySelector<HTMLElement>(".chip-row")?.offsetHeight ?? 0;
    fitted = Math.max(1, Math.floor((root.clientHeight - chip - chrome) / perLine));
  });
</script>

<svelte:window bind:innerHeight={windowHeight} />

<!-- Running text is pinned like live captions: the frame keeps its place and size through
     silence, so the reader's eyes never have to find it again. -->
<section bind:this={section} class="flow" aria-label="Live captions" data-testid="flow-text">
  <StatusPill {status} />
  {#if showSourceParagraph}
    <div class="para source" style:--flow-lines={lines}>
      <CaptionViewport
        snap="line"
        height="calc(var(--flow-lines) * var(--flow-line-height) * 1em)"
        {reducedMotion}
      >
        <p>
          {#each flow.source as piece, index (`${index}-${piece.key}`)}<span class:live={piece.kind === "live"} lang={piece.kind === "live" ? undefined : "zh"} data-block={piece.key}>{piece.text}</span>{/each}
        </p>
      </CaptionViewport>
    </div>
  {/if}
  <div class="para english" style:--flow-lines={lines}>
    <CaptionViewport
      snap="line"
      height="calc(var(--flow-lines) * var(--flow-line-height) * 1em)"
      {reducedMotion}
    >
      <p>
        {#if flow.english.length > 0}
          {#each flow.english as piece, index (`${index}-${piece.key}`)}{index > 0 ? " " : ""}<span class={piece.kind} data-block={piece.key}>{piece.text}</span>{/each}{#if streaming}<span class="caret" aria-hidden="true"></span>{/if}
        {:else if !status && flow.source.length === 0}
          <span class="waiting">Waiting for speech</span>
        {/if}
      </p>
    </CaptionViewport>
  </div>
</section>

<style>
  .flow {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: none;
    gap: var(--space-2);
    width: 100%;
    max-width: 960px;
    padding: calc(var(--space-4) + var(--status-h) + var(--space-2)) var(--space-6) 18px;
    background: var(--ov-bar-bg);
    border-radius: var(--radius-overlay);
  }

  /* One fixed-height box per paragraph, cut to whole lines at the top. The fit clamp needs the
   * real heights, so a paragraph never shrinks to fit the window. */
  .para {
    --flow-line-height: 1.35;
    flex: none;
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
