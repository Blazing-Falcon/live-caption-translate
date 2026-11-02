<script lang="ts">
  import type { StatusLine } from "../lib/captions";

  let { status }: { status: StatusLine | null } = $props();
</script>

{#if status}
  <p class="status" data-kind={status.kind}>
    <span class="mark" aria-hidden="true">!</span>
    <span class="text">{status.text}</span>
  </p>
{/if}

<style>
  /* In the space the captions leave for it (`--status-h`), so a status never changes their height. */
  .status {
    position: absolute;
    top: var(--space-4);
    left: 18px;
    max-width: calc(100% - 36px);
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    max-height: var(--status-h);
    margin: 0;
    padding: 5px 12px 5px 6px;
    border-radius: 14px;
    background: var(--cap-tag-bg);
    font: 700 15px / 18px var(--font-body);
    color: var(--cap-tag-ink);
  }

  .text {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  .mark {
    flex: none;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--cap-tag-ink);
    color: var(--cap-tag-bg);
    font-weight: 800;
    font-size: 13px;
  }
</style>
