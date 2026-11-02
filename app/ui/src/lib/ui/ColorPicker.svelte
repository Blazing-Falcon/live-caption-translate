<script lang="ts">
  import type { ColorPreset } from "../colors";

  let {
    name,
    label,
    presets,
    value,
    onchange,
  }: {
    name: string;
    label: string;
    presets: readonly ColorPreset[];
    value: string;
    onchange: (value: string) => unknown;
  } = $props();

  const custom = $derived(!presets.some((preset) => preset.value === value));
</script>

<div class="wells" role="radiogroup" aria-label={label}>
  {#each presets as preset (preset.value)}
    <label class="well" title={preset.name}>
      <input
        type="radio"
        {name}
        value={preset.value}
        aria-label={preset.name}
        checked={value === preset.value}
        onchange={() => onchange(preset.value)}
      />
      <span style:background={preset.value}></span>
    </label>
  {/each}
  <input
    class="custom"
    class:chosen={custom}
    type="color"
    aria-label="Custom color"
    title="Custom color"
    {value}
    onchange={(event) => onchange(event.currentTarget.value)}
  />
</div>

<style>
  .wells {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .well {
    position: relative;
    cursor: pointer;
  }

  .well input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
    cursor: pointer;
  }

  .well span {
    display: block;
    width: 28px;
    height: 28px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: 50%;
  }

  .well input:checked + span,
  .custom.chosen {
    box-shadow: 0 0 0 2px var(--paper), 0 0 0 4.5px var(--pen-text);
  }

  .well input:focus-visible + span,
  .custom:focus-visible {
    outline: 2.5px solid var(--pen-text);
    outline-offset: 6px;
  }

  .custom {
    appearance: none;
    width: 28px;
    height: 28px;
    padding: 0;
    border: var(--stroke-w) dashed var(--ink-3);
    border-radius: 50%;
    background: conic-gradient(var(--hl-rose), var(--hl-lemon), var(--hl-sage), var(--hl-mist), var(--hl-haze), var(--hl-rose));
    cursor: pointer;
  }

  .custom::-webkit-color-swatch-wrapper {
    padding: 0;
  }

  .custom::-webkit-color-swatch {
    border: 0;
    border-radius: 50%;
    opacity: 0;
  }

  .custom.chosen {
    border-style: solid;
    border-color: var(--stroke);
  }

  .custom.chosen::-webkit-color-swatch {
    opacity: 1;
  }
</style>
