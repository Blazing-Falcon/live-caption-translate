<script lang="ts">
  import { ageOpacity, type CaptionBlock as Block } from "../lib/captions";

  interface Props {
    block: Block;
    age?: number;
    showSource?: boolean;
  }

  let { block, age = 0, showSource = true }: Props = $props();

  const sourceVisible = $derived(
    block.source !== "" &&
      (showSource || block.state === "pending" || block.state === "skipped" || block.state === "failed"),
  );
  const showEnglish = $derived(block.english !== "" || block.state === "streaming");
</script>

<article
  class="block"
  class:skipped={block.state === "skipped"}
  data-block={block.key}
  data-state={block.state}
  style:--age-opacity={ageOpacity(age)}
>
  {#if block.state === "english"}
    <p class="english main">{block.source}</p>
    <p class="label">English · Not translated</p>
  {:else}
    {#if sourceVisible}
      <p class="source" lang="zh">{block.source}</p>
    {/if}
    {#if block.live !== ""}
      <p class="source heard">{block.live}</p>
    {/if}
    <div class="english-row">
      {#if block.preview !== null}
        <p class="english preview">{block.preview}</p>
      {:else if showEnglish}
        <p class="english">{block.english}{#if block.state === "streaming"}<span class="caret" aria-hidden="true"></span>{/if}</p>
      {:else if block.state === "pending"}
        <p class="english dots" aria-hidden="true">···</p>
        <span class="sr-only">Translating</span>
      {/if}
    </div>
    {#if block.state === "skipped"}
      <p class="label">Skipped to catch up</p>
    {:else if block.state === "failed"}
      <p class="label warning">Translation failed · {block.reason ?? "Could not translate this line"}</p>
    {/if}
  {/if}
</article>

<style>
  .block {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    /* In the box, not between boxes: a pruned block must take its whole stride with it. */
    padding-bottom: var(--panel-gap);
    opacity: var(--age-opacity, 1);
    transition: opacity var(--fade-ms) linear;
    min-width: 0;
  }

  .block.skipped {
    opacity: calc(var(--age-opacity, 1) * 0.6);
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  /* Every caption size is one step from the setting: an older block dims but never shrinks, so
   * the reader's eye keeps its place on the panel. */
  .english {
    font-family: var(--cap-font);
    font-size: calc(var(--font-px, 26) * 1px);
    font-weight: var(--cap-weight);
    line-height: 1.3;
    color: var(--ov-text);
    text-shadow: var(--ov-halo);
  }

  /* The first preview must not add a row: the English row keeps its height from the first live
   * word, so the block below does not move when the preview arrives. */
  .english-row {
    min-height: calc(var(--font-px, 26) * 1.3 * 1px);
  }

  .source {
    font-family: var(--font-cjk);
    /* Smaller Chinese than this is not readable, whatever the English size. */
    font-size: max(12px, calc(var(--font-px, 26) * 1px * var(--source-scale)));
    line-height: 1.45;
    color: var(--ov-text-secondary);
    text-shadow: var(--ov-halo);
  }

  /* Words still being recognized, not a settled line: dimmer than the Chinese of a finished
   * clause, so the reader can tell what the recognizer may still change. */
  .source.heard {
    color: var(--ov-dim);
  }

  .dots {
    color: var(--ov-pending);
  }

  /* English that the next translation may still change. */
  .preview {
    color: var(--cap-preview);
  }

  .label {
    font-size: var(--text-meta);
    letter-spacing: var(--meta-tracking);
    text-transform: uppercase;
    color: var(--ov-meta);
  }

  .label.warning {
    color: var(--ov-warning);
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
