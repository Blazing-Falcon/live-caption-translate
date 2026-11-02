<script lang="ts">
  import type { CaptionBlock, StatusLine } from "../lib/captions";
  import StatusPill from "./StatusPill.svelte";

  interface Props {
    blocks: readonly CaptionBlock[];
    status?: StatusLine | null;
    showSource?: boolean;
  }

  let { blocks, status = null, showSource = true }: Props = $props();

  /** Like film subtitles: the newest block, with the one before it above and dimmed. */
  const shown = $derived(blocks.slice(-2));
</script>

<section class="subtitles" aria-label="Live captions" data-testid="subtitles">
  <StatusPill {status} />
  {#each shown as block, index (block.key)}
    {@const older = index < shown.length - 1}
    <div class="block" class:older data-block={block.key} data-state={block.state}>
      {#if block.state === "english"}
        <p class="english"><span class="line">{block.source}</span></p>
      {:else}
        {#if block.source !== "" && (showSource || block.state === "pending" || block.state === "failed")}
          <p class="source" lang="zh"><span class="line">{block.source}</span></p>
        {/if}
        {#if block.live !== ""}
          <p class="source heard"><span class="line">{block.live}</span></p>
        {/if}
        {#if block.preview !== null}
          <p class="english preview"><span class="line">{block.preview}</span></p>
        {:else if block.english !== ""}
          <p class="english"><span class="line">{block.english}</span></p>
        {:else if block.state === "pending" || block.state === "streaming"}
          <p class="english dots" aria-hidden="true"><span class="line">···</span></p>
        {/if}
        {#if block.state === "failed"}
          <p class="label"><span class="line">Translation failed · {block.reason ?? "Could not translate this line"}</span></p>
        {/if}
      {/if}
    </div>
  {:else}
    {#if !status}<p class="english waiting"><span class="line">Waiting for speech</span></p>{/if}
  {/each}
</section>

<style>
  .subtitles {
    position: relative;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    align-items: center;
    gap: var(--space-3);
    height: 100%;
    padding: calc(var(--space-4) + var(--status-h) + var(--space-2)) var(--space-4) var(--space-4);
    text-align: center;
  }

  .block {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    max-width: 100%;
    transition: opacity var(--fade-ms) linear;
  }

  .older {
    opacity: 0.62;
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  /* One box per wrapped line, like broadcast subtitles; an outline card has no box. */
  .line {
    padding: 1px 10px;
    border-radius: 8px;
    background: var(--ov-bar-bg);
    -webkit-box-decoration-break: clone;
    box-decoration-break: clone;
  }

  .english {
    font-family: var(--cap-font);
    font-size: calc(var(--font-px, 26) * 1px);
    font-weight: var(--cap-weight);
    line-height: 1.5;
    color: var(--ov-text);
    text-shadow: var(--ov-halo);
  }

  .source {
    font-family: var(--font-cjk);
    font-size: max(12px, calc(var(--font-px, 26) * 1px * var(--source-scale)));
    line-height: 1.6;
    color: var(--ov-text-secondary);
    text-shadow: var(--ov-halo);
  }

  .heard {
    color: var(--ov-dim);
  }

  .preview {
    color: var(--cap-preview);
  }

  .dots,
  .waiting {
    color: var(--ov-pending);
  }

  .label {
    font-size: var(--text-meta);
    letter-spacing: var(--meta-tracking);
    color: var(--ov-warning);
    text-shadow: var(--ov-halo);
  }
</style>
