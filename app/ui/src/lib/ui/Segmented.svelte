<script lang="ts" generics="T extends string">
  let {
    name,
    label,
    options,
    value,
    onchange,
    disabled = false,
  }: {
    name: string;
    label: string;
    options: readonly { value: T; label: string }[];
    value: T;
    /** A failed save leaves `value` as it was; the radios then show it again. */
    onchange: (value: T) => unknown;
    disabled?: boolean;
  } = $props();

  async function choose(input: HTMLInputElement, next: T): Promise<void> {
    await onchange(next);
    for (const radio of input.closest(".segmented")?.querySelectorAll("input") ?? []) radio.checked = radio.value === value;
  }
</script>

<div class="segmented" role="radiogroup" aria-label={label}>
  {#each options as option (option.value)}
    <label>
      <input
        type="radio"
        {name}
        value={option.value}
        checked={value === option.value}
        {disabled}
        onchange={(event) => choose(event.currentTarget, option.value)}
      />
      <span>{option.label}</span>
    </label>
  {/each}
</div>

<style>
  .segmented {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 2px;
    padding: 3px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-pill);
    background: var(--paper-2);
  }

  label {
    position: relative;
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

  span {
    display: block;
    padding: 4px 13px;
    border-radius: var(--r-pill);
    color: var(--ink-2);
    font-weight: 700;
    font-size: 13.5px;
  }

  input:checked + span {
    background: var(--pen-soft);
    color: var(--ink);
    outline: 2px solid var(--pen-text);
    outline-offset: -2px;
  }

  input:focus-visible + span {
    box-shadow: 0 0 0 2.5px var(--paper), 0 0 0 5px var(--pen-text);
  }

  input:disabled + span {
    opacity: 0.55;
  }
</style>
