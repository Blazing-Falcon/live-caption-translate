<script lang="ts">
  import { ageOpacity, type CaptionBlock, type StatusLine } from "../lib/captions";

  interface Props {
    block?: CaptionBlock | null;
    status?: StatusLine | null;
    age?: number;
    showSource?: boolean;
  }

  let {
    block = null,
    status = null,
    age = 0,
    showSource = true,
  }: Props = $props();

  // A live line with nothing to show yet is not rendered at all, so it reserves no space.
  const sourceVisible = $derived(
    block !== null && block.source !== "" && (block.sealed || block.state !== "open" || showSource),
  );
  const englishText = $derived(
    block !== null && (block.state === "final" || block.state === "streaming") ? block.english : "",
  );
  const showEnglish = $derived(block !== null && (englishText !== "" || block.state === "streaming"));
</script>

{#if status}
  <p class="status" data-kind={status.kind}>
    <span class="hollow" aria-hidden="true"></span>
    <span>{status.text}</span>
  </p>
{:else if block}
  <article
    class="line"
    class:skipped={block.state === "skipped"}
    data-id={block.key}
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
      {#if block.preview}
        <p class="english preview">{block.preview}</p>
      {:else if showEnglish}
        <p class="english">{englishText}{#if block.state === "streaming"}<span class="caret" aria-hidden="true"></span>{/if}</p>
      {:else if block.state === "pending"}
        <p class="english dots" aria-hidden="true">···</p>
        <span class="sr-only">Translating</span>
      {/if}
      {#if block.state === "skipped"}
        <p class="label">Skipped to catch up</p>
      {:else if block.state === "failed"}
        <p class="label warning">Translation failed · {block.reason ?? "Could not translate this line"}</p>
      {/if}
    {/if}
  </article>
{/if}

<style>
  .line {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    opacity: var(--age-opacity, 1);
    transition: opacity var(--fade-ms) linear;
    animation: slide-up var(--slide-ms) ease-out;
    min-width: 0;
  }

  .line.skipped {
    opacity: calc(var(--age-opacity, 1) * 0.6);
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  /* Every caption size is one step from the setting: an older line dims but never shrinks, so
   * the reader's eye keeps its place on the panel. */
  .english {
    font-size: calc(var(--font-px, 26) * 1px);
    font-weight: 500;
    line-height: 1.3;
    color: var(--ov-text);
    text-shadow: var(--ov-halo);
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
    color: var(--ov-text-secondary);
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

  .status {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0;
    font-size: 15px;
    color: var(--ov-text-secondary);
    animation: slide-up var(--slide-ms) ease-out;
  }

  .hollow {
    flex: none;
    width: 7px;
    height: 7px;
    border: 1.5px solid var(--ov-meta);
    border-radius: 50%;
  }

  @keyframes slide-up {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
  }

  @keyframes caret-blink {
    50% {
      opacity: 0;
    }
  }
</style>
