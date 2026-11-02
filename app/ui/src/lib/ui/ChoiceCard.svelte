<script lang="ts">
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  let {
    name,
    value,
    checked,
    icon,
    title,
    description,
    meta = "",
    disabled = false,
    testid,
    onchange,
  }: {
    name: string;
    value: string;
    checked: boolean;
    icon: IconName;
    title: string;
    description: string;
    meta?: string;
    disabled?: boolean;
    testid?: string;
    /** A failed save leaves `checked` as it was; the radio then shows it again. */
    onchange: () => unknown;
  } = $props();

  async function choose(input: HTMLInputElement): Promise<void> {
    await onchange();
    input.checked = checked;
  }
</script>

<label class="choice" class:disabled data-testid={testid}>
  <input type="radio" {name} {value} {checked} {disabled} onchange={(event) => choose(event.currentTarget)} />
  <span class="icon"><Icon name={icon} size={22} /></span>
  <span class="title">{title}</span>
  <span class="description">{description}</span>
  {#if meta}<span class="meta">{meta}</span>{/if}
</label>

<style>
  .choice {
    position: relative;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 2px 12px;
    padding: 12px 14px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-ctl);
    background: var(--card);
    cursor: pointer;
  }

  input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
    cursor: pointer;
  }

  .icon {
    grid-row: span 3;
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 12px;
    background: var(--paper-2);
  }

  .title {
    font-weight: 700;
    font-size: 15.5px;
  }

  .description {
    color: var(--ink-2);
    font-size: 13.5px;
    line-height: 1.45;
  }

  .meta {
    color: var(--ink-3);
    font-size: 12.5px;
    font-weight: 800;
  }

  .choice:has(input:checked) {
    border-color: var(--pen-text);
    background: var(--pen-soft);
    box-shadow: 0 0 0 1px var(--pen-text);
  }

  .choice:has(input:checked) .icon {
    background: var(--card);
  }

  .choice:has(input:focus-visible) {
    outline: 2.5px solid var(--pen-text);
    outline-offset: 3px;
  }

  .disabled {
    opacity: 0.55;
    cursor: default;
  }
</style>
