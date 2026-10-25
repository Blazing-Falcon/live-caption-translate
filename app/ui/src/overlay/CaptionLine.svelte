<script lang="ts">
  import { ageOpacity, isHiddenLive, type CaptionLineView, type StatusLine } from "../lib/captions";
  import { languageLabel } from "../lib/settings";
  import type { CaptionLine } from "../lib/types";

  interface Props {
    line?: CaptionLineView | null;
    status?: StatusLine | null;
    age?: number;
    showSource?: boolean;
    liveSource?: boolean;
  }

  let {
    line = null,
    status = null,
    age = 0,
    showSource = true,
    liveSource = true,
  }: Props = $props();

  // Labels are uppercased by CSS; the DOM keeps sentence case so screen readers do not spell them out.
  const sentenceCase = (value: string): string => value.charAt(0) + value.slice(1).toLowerCase();

  const langName = $derived(sentenceCase(languageLabel(line?.lang)));
  const otherLabel = $derived(`${langName} · Not translated`);
  // A live line with nothing to show yet is not rendered at all, so it reserves no space.
  const hidden = $derived(line !== null && isHiddenLive(line, liveSource));
  const sourceVisible = $derived(
    line !== null &&
      (line.state === "live"
        ? liveSource && line.source !== ""
        : line.state === "pending" || line.state === "skipped" || line.state === "failed" || showSource),
  );
  const englishText = $derived(
    line !== null && (line.state === "final" || line.state === "streaming") ? line.english : "",
  );
  const showEnglish = $derived(line !== null && (englishText !== "" || line.state === "streaming"));
</script>

{#if status}
  <p class="status" data-kind={status.kind}>
    <span class="hollow" aria-hidden="true"></span>
    <span>{status.text}</span>
  </p>
{:else if line && !hidden}
  <article
    class="line"
    class:skipped={line.state === "skipped"}
    data-id={line.id}
    data-state={line.state}
    style:--age-opacity={ageOpacity(age)}
  >
    {#if line.state === "english"}
      <p class="english main">{line.source}</p>
      <p class="label">English · Not translated</p>
    {:else if line.state === "other"}
      <p class="other main" lang={line.lang ?? undefined}>{line.source}</p>
      <p class="label">{otherLabel}</p>
    {:else}
      {#if sourceVisible}
        {#if line.swap}
          <p class="source swapping" lang="zh">
            <span class="swap-out" aria-hidden="true">{line.swap.from}</span>
            <span class="swap-in">{line.source}</span>
          </p>
        {:else}
          <p class="source" class:heard={line.state === "live"} lang={line.state === "live" ? undefined : "zh"}>{line.source}</p>
        {/if}
      {/if}
      {#if line.preview}
        <p class="english preview">{line.preview}</p>
      {:else if showEnglish}
        <p class="english">{englishText}{#if line.state === "streaming"}<span class="caret" aria-hidden="true"></span>{/if}</p>
      {:else if line.state === "pending"}
        <p class="english dots" aria-hidden="true">···</p>
        <span class="sr-only">Translating</span>
      {/if}
      {#if line.state === "skipped"}
        <p class="label">Skipped to catch up</p>
      {:else if line.state === "failed"}
        <p class="label warning">Translation failed · {line.reason ?? "Could not translate this line"}</p>
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

  .other {
    font-size: calc(var(--font-px, 26) * 1px);
    line-height: 1.35;
    color: var(--ov-text-secondary);
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

  /* The corrected Chinese fades in over the text the viewer was reading, so a larger correction
   * reads as a change rather than as a flicker. */
  .swapping {
    display: grid;
  }

  .swapping > span {
    grid-area: 1 / 1;
  }

  .swap-out {
    animation: swap-out var(--swap-ms) ease-in forwards;
  }

  .swap-in {
    animation: swap-in var(--swap-ms) ease-out;
  }

  @keyframes swap-out {
    to {
      opacity: 0;
    }
  }

  @keyframes swap-in {
    from {
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .swap-out,
    .swap-in {
      animation: none;
    }
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
