<script lang="ts">
  import type { StatusLine } from "../lib/captions";

  let { status }: { status: StatusLine | null } = $props();
</script>

{#if status}
  <p class="status" data-kind={status.kind}>
    <span class="hollow" aria-hidden="true"></span>
    <span>{status.text}</span>
  </p>
{/if}

<style>
  /* Positioned over the text, so a status never changes the height of the captions below it. */
  .status {
    position: absolute;
    top: var(--space-4);
    left: 18px;
    right: 18px;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 0;
    padding: 4px 8px;
    border-radius: var(--radius-control);
    background: var(--ov-chip-bg);
    font-size: 15px;
    color: var(--ov-text-secondary);
  }

  .hollow {
    flex: none;
    width: 7px;
    height: 7px;
    border: 1.5px solid var(--ov-meta);
    border-radius: 50%;
  }
</style>
