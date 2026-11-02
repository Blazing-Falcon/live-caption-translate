<script lang="ts">
  let {
    id,
    label,
    min,
    max,
    step = 1,
    value,
    format,
    oninput,
    onchange,
  }: {
    id: string;
    label: string;
    min: number;
    max: number;
    step?: number;
    value: number;
    format: (value: number) => string;
    /** Each step while dragging. */
    oninput?: (value: number) => void;
    /** The value to keep, when the drag ends. */
    onchange: (value: number) => void;
  } = $props();
</script>

<div class="slider">
  <input
    {id}
    type="range"
    aria-label={label}
    {min}
    {max}
    {step}
    {value}
    oninput={(event) => oninput?.(Number(event.currentTarget.value))}
    onchange={(event) => onchange(Number(event.currentTarget.value))}
  />
  <output for={id}>{format(value)}</output>
</div>

<style>
  .slider {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  input {
    flex: 1;
    min-width: 0;
    accent-color: var(--pen-text);
  }

  output {
    min-width: 56px;
    text-align: right;
    font-weight: 800;
    font-variant-numeric: tabular-nums;
  }
</style>
