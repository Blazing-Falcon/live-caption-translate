<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  let {
    tone = "lemon",
    icon = "info",
    role,
    testid,
    children,
  }: {
    tone?: "lemon" | "sage" | "rose" | "mist";
    icon?: IconName;
    role?: "note" | "status" | "alert";
    testid?: string;
    children: Snippet;
  } = $props();
</script>

<div class="note {tone}" {role} data-testid={testid}>
  <Icon name={icon} size={20} />
  <div class="text">{@render children()}</div>
</div>

<style>
  .note {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 12px 14px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: 14px;
    color: var(--ink);
    font-size: 14px;
    line-height: 1.5;
    box-shadow: var(--sticker-shadow);
  }

  .text {
    min-width: 0;
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
</style>
