<script lang="ts">
  import { useSession } from "../lib/context";

  const session = useSession();
  const { notices } = session;
</script>

<div class="notices" aria-live="polite" data-testid="notices">
  {#each $notices as notice (notice.id)}
    <div class="notice" class:error={notice.kind === "error"} role={notice.kind === "error" ? "alert" : "status"}>
      <span>{notice.text}</span>
      <button type="button" aria-label="Dismiss message" onclick={() => session.dismissNotice(notice.id)}>Dismiss</button>
    </div>
  {/each}
</div>
