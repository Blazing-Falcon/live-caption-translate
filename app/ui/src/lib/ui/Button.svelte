<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

  type Props = HTMLButtonAttributes & {
    variant?: "primary" | "normal" | "quiet" | "danger";
    size?: "small" | "normal" | "big";
    /** Stays focusable and announced while ignoring clicks, unlike `disabled`. */
    busy?: boolean;
    children: Snippet;
  };

  let { variant = "normal", size = "normal", busy = false, type = "button", children, onclick, ...rest }: Props = $props();
</script>

<button
  {...rest}
  {type}
  class="btn {variant} {size}"
  aria-disabled={busy || undefined}
  onclick={(event) => {
    if (busy) return;
    onclick?.(event);
  }}
>
  {@render children()}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 36px;
    padding: 6px 16px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-pill);
    background: var(--card);
    color: var(--ink);
    font: 700 14px / 1.2 var(--font-body);
    white-space: nowrap;
    cursor: pointer;
    box-shadow: var(--sticker-shadow-small);
  }

  .btn:active:not([aria-disabled]):not(:disabled) {
    transform: translate(2px, 2px);
    box-shadow: none;
  }

  .btn:focus-visible {
    outline: 2.5px solid var(--pen-text);
    outline-offset: 3px;
  }

  .primary {
    background: var(--pen);
    color: var(--pen-ink);
  }

  .quiet {
    border-color: transparent;
    background: transparent;
    color: var(--pen-text);
    box-shadow: none;
    padding-inline: 8px;
  }

  .danger {
    color: var(--bad);
    border-color: var(--bad);
  }

  .small {
    min-height: 30px;
    padding: 3px 12px;
    font-size: 13px;
  }

  .big {
    min-height: 44px;
    padding: 8px 24px;
    font-size: 16px;
  }

  .btn[aria-disabled],
  .btn:disabled {
    opacity: 0.55;
    cursor: default;
  }
</style>
