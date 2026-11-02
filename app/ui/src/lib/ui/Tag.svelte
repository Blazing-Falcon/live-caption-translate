<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    tone = "plain",
    testid,
    children,
  }: { tone?: "plain" | "lemon" | "sage" | "rose" | "mist" | "haze"; testid?: string; children: Snippet } = $props();
</script>

<span class="tag {tone}" data-testid={testid}>{@render children()}</span>

<style>
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 10px;
    border-radius: var(--r-pill);
    border: 1.5px solid var(--line);
    background: var(--paper-2);
    color: var(--ink-2);
    font-size: 12.5px;
    font-weight: 800;
    white-space: nowrap;
  }

  .lemon,
  .sage,
  .rose,
  .mist,
  .haze {
    border-color: transparent;
    color: var(--ink);
  }

  .lemon {
    background: var(--hl-lemon);
  }

  .sage {
    background: var(--hl-sage);
  }

  .rose {
    background: var(--hl-rose);
  }

  .mist {
    background: var(--hl-mist);
  }

  .haze {
    background: var(--hl-haze);
  }
</style>
