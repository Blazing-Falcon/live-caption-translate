<script lang="ts">
  let {
    checked,
    label,
    disabled = false,
    onchange,
    testid,
  }: {
    checked: boolean;
    /** The visible title, read out as the switch's name. */
    label: string;
    disabled?: boolean;
    onchange: (checked: boolean) => void;
    testid?: string;
  } = $props();
</script>

<button
  type="button"
  class="switch"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  data-testid={testid}
  onclick={() => onchange(!checked)}
></button>

<style>
  .switch {
    flex: none;
    position: relative;
    width: 48px;
    height: 28px;
    padding: 0;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-pill);
    background: var(--paper-2);
    cursor: pointer;
  }

  .switch::after {
    content: "";
    position: absolute;
    top: 50%;
    left: 3px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--ink-3);
    transform: translateY(-50%);
    transition: left 160ms ease, background 160ms ease;
  }

  .switch[aria-checked="true"] {
    background: var(--hl-sage);
    border-color: var(--good);
  }

  .switch[aria-checked="true"]::after {
    left: 23px;
    background: var(--good);
  }

  .switch:focus-visible {
    outline: 2.5px solid var(--pen-text);
    outline-offset: 3px;
  }

  .switch:disabled {
    opacity: 0.55;
    cursor: default;
  }
</style>
