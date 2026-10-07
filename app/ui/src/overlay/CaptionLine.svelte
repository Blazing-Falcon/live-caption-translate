<script lang="ts">
  import { ageOpacity, type OverlayStyle, type StatusLine } from "../lib/captions";
  import { languageLabel, normalizeLang } from "../lib/settings";
  import type { CaptionLine } from "../lib/types";

  interface Props {
    line?: CaptionLine | null;
    status?: StatusLine | null;
    variant?: OverlayStyle;
    age?: number;
    showSource?: boolean;
    fading?: boolean;
  }

  let { line = null, status = null, variant = "bar", age = 0, showSource = true, fading = false }: Props = $props();

  const TOGGLEABLE = new Set(["ja", "ko"]);

  // Labels are uppercased by CSS; the DOM keeps sentence case so screen readers do not spell them out.
  const sentenceCase = (value: string): string => value.charAt(0) + value.slice(1).toLowerCase();

  const langName = $derived(sentenceCase(languageLabel(line?.lang)));
  const otherLabel = $derived(
    TOGGLEABLE.has(normalizeLang(line?.lang) ?? "")
      ? `${langName} · Turn on in settings to translate`
      : `${langName} · Not translated`,
  );
  const sourceVisible = $derived(
    line !== null && (line.state === "pending" || line.state === "skipped" || line.state === "failed" || showSource),
  );
</script>

{#if status}
  <p class="status" data-kind={status.kind}>
    <span class="hollow" aria-hidden="true"></span>
    <span>{status.text}</span>
  </p>
{:else if line}
  <article
    class="line"
    class:older={age > 0}
    class:fading
    class:skipped={line.state === "skipped"}
    data-id={line.id}
    data-state={line.state}
    style:--age-opacity={ageOpacity(variant, age)}
  >
    {#if line.state === "english"}
      <p class="english main">{line.source}</p>
      <p class="label">English · Not translated</p>
    {:else if line.state === "other"}
      <p class="other main" lang={line.lang ?? undefined}>{line.source}</p>
      <p class="label">{otherLabel}</p>
    {:else}
      {#if sourceVisible}
        <p class="source" lang="zh">{line.source}</p>
      {/if}
      {#if line.state === "pending"}
        <p class="english dots" aria-hidden="true">···</p>
        <span class="sr-only">Translating</span>
      {:else if line.state === "streaming"}
        <p class="english">{line.english}<span class="caret" aria-hidden="true"></span></p>
      {:else if line.state === "final"}
        <p class="english">{line.english}</p>
      {:else if line.state === "skipped"}
        <p class="label">Skipped to catch up</p>
      {:else if line.state === "failed"}
        <p class="label warning">Translation failed · {line.reason ?? "Could not translate this line"}</p>
      {/if}
    {/if}
  </article>
{/if}

<style>
  .line {
    --line-scale: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    opacity: var(--age-opacity, 1);
    transition: opacity var(--fade-ms) linear;
    animation: slide-up var(--slide-ms) ease-out;
    min-width: 0;
  }

  .line.older {
    --line-scale: var(--older-scale);
  }

  .line.skipped {
    opacity: calc(var(--age-opacity, 1) * 0.6);
  }

  .line.fading {
    opacity: 0;
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .english {
    font-size: calc(var(--font-px, 26) * 1px * var(--line-scale));
    font-weight: 500;
    line-height: 1.3;
    color: var(--ov-text);
  }

  .other {
    font-size: calc(var(--font-px, 26) * 1px * var(--line-scale) * var(--older-scale));
    line-height: 1.35;
    color: var(--ov-text-secondary);
  }

  .source {
    font-family: var(--font-cjk);
    font-size: calc(var(--font-px, 26) * 1px * var(--source-scale) * var(--line-scale));
    line-height: 1.45;
    color: var(--ov-text-secondary);
  }

  .dots {
    color: var(--ov-pending);
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
