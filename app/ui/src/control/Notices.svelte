<script lang="ts">
  import { useSession } from "../lib/context";
  import Icon from "../lib/ui/Icon.svelte";

  const session = useSession();
  const { notices } = session;
</script>

<div class="notices" aria-live="polite" data-testid="notices">
  {#each $notices as notice (notice.id)}
    <div class="notice" data-kind={notice.kind} role={notice.kind === "error" ? "alert" : "status"}>
      <Icon name={notice.kind === "info" ? "info" : "alert"} size={18} />
      <span class="text">{notice.text}</span>
      <button type="button" aria-label="Dismiss message" onclick={() => session.dismissNotice(notice.id)}>Dismiss</button>
    </div>
  {/each}
</div>

<style>
  .notices {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 760px;
  }

  .notices:empty {
    display: none;
  }

  .notice {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: 14px;
    background: var(--hl-mist);
    color: var(--ink);
    font-size: 14px;
  }

  .notice[data-kind="warning"] {
    background: var(--hl-lemon);
  }

  .notice[data-kind="error"] {
    background: var(--hl-rose);
  }

  .text {
    flex: 1;
    min-width: 0;
  }

  button {
    border: 0;
    background: transparent;
    color: var(--pen-text);
    font: 700 13px / 1.4 var(--font-body);
    cursor: pointer;
  }
</style>
