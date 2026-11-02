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
    onchange: (value: T) => void;
    disabled?: boolean;
  } = $props();
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
        onchange={() => onchange(option.value)}
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
    opacity: 0;
    pointer-events: none;
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
