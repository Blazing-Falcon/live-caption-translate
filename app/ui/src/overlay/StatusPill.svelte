<script lang="ts">
  import type { StatusLine } from "../lib/captions";

  let { status }: { status: StatusLine | null } = $props();
</script>

{#if status}
  <p class="status" data-kind={status.kind}>
    <span class="hollow" aria-hidden="true"></span>
    <span class="text">{status.text}</span>
  </p>
{/if}

<style>
  /* In the space the captions leave for it (`--status-h`), so a status never changes their height. */
  .status {
    position: absolute;
    top: var(--space-4);
    left: 18px;
    right: 18px;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    max-height: var(--status-h);
    margin: 0;
    padding: 4px 8px;
    border-radius: var(--radius-control);
    background: var(--ov-chip-bg);
    font-size: 15px;
    line-height: 18px;
    color: var(--ov-text-secondary);
  }

  .text {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  .hollow {
    flex: none;
    width: 7px;
    height: 7px;
    border: 1.5px solid var(--ov-meta);
    border-radius: 50%;
  }
</style>
