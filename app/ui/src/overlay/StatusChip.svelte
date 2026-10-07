<script lang="ts">
  import type { ChipMessage } from "../lib/captions";

  let { chip = null }: { chip?: ChipMessage | null } = $props();
</script>

{#if chip}
  {#key chip.id}
    <div class="chip" role="status" data-testid="status-chip">
      <span class="dot" aria-hidden="true"></span>
      <span>{chip.text}</span>
    </div>
  {/key}
{/if}

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    padding: 6px 10px;
    background: var(--ov-chip-bg);
    border-radius: var(--radius-control);
    font-size: var(--text-help);
    color: var(--ov-chip-text);
    animation: chip-in var(--slide-ms) ease-out;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ov-accent);
  }

  @keyframes chip-in {
    from {
      opacity: 0;
    }
  }
</style>
