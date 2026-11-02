<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    title,
    help,
    control,
    stacked = false,
    id,
    testid,
  }: {
    title: Snippet | string;
    help?: Snippet | string;
    control?: Snippet;
    /** The control goes under the text, full width: sliders, pickers and fields. */
    stacked?: boolean;
    /** Names the help text so a control can point at it with aria-describedby. */
    id?: string;
    testid?: string;
  } = $props();
</script>

<div class="row" class:stacked data-testid={testid}>
  <div class="text">
    <span class="title">{#if typeof title === "string"}{title}{:else}{@render title()}{/if}</span>
    {#if help}
      <span class="help" id={id ? `${id}-help` : undefined}>{#if typeof help === "string"}{help}{:else}{@render help()}{/if}</span>
    {/if}
  </div>
  {#if control}<div class="control">{@render control()}</div>{/if}
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 18px;
    border-top: 1.5px dashed var(--line);
  }

  :global(.section > h3) + .row,
  .row:first-child {
    border-top: 0;
  }

  .stacked {
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
  }

  .text {
    flex: 1 1 14em;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .title {
    font-weight: 700;
  }

  .help {
    color: var(--ink-2);
    font-size: 13.5px;
    line-height: 1.45;
  }

  .control {
    flex: 0 1 auto;
    min-width: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }

  /* In a column the 14em basis would become a height. */
  .stacked .text {
    flex: none;
  }

  .stacked .control {
    justify-content: flex-start;
  }
</style>
