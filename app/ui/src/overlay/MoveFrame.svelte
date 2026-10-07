<script lang="ts">
  interface Props {
    binding?: string;
    onlock?: () => void;
  }

  let { binding = "", onlock }: Props = $props();
</script>

<div class="frame" data-testid="move-frame" data-tauri-drag-region>
  <div class="toolbar">
    <span>Move or resize, then lock</span>
    {#if binding}
      <kbd>{binding}</kbd>
    {/if}
    <button type="button" onclick={() => onlock?.()}>Lock</button>
  </div>
</div>

<style>
  .frame {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: var(--space-2);
    border: 1.5px dashed var(--ov-outline);
    border-radius: var(--radius-overlay);
    cursor: move;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 8px 12px 8px 14px;
    background: var(--ov-move-bg);
    border-radius: var(--radius-card);
    font-size: var(--text-label);
    color: var(--ov-text-bright);
    cursor: default;
  }

  kbd {
    padding: 2px 6px;
    border: 1px solid var(--ov-key-border);
    border-radius: var(--space-1);
    font: inherit;
    font-size: var(--text-help);
    color: var(--ov-chip-text);
  }

  button {
    min-height: 32px;
    padding: 6px 12px;
    border: 0;
    border-radius: var(--radius-control);
    background: var(--ov-accent);
    color: var(--ov-button-text);
    font-weight: 500;
    cursor: pointer;
  }
</style>
