<script lang="ts">
  let { combo }: { combo: string } = $props();

  const keys = $derived(combo.split("+").map((key) => key.trim()).filter(Boolean));
</script>

<span class="keys">
  {#each keys as key, index (index)}
    {#if index > 0}<span class="plus" aria-hidden="true">+</span>{/if}<kbd>{key}</kbd>
  {/each}
</span>

<style>
  .keys {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    white-space: nowrap;
  }

  kbd {
    padding: 1px 7px;
    border: 1.5px solid var(--stroke);
    border-bottom-width: 3px;
    border-radius: 7px;
    background: var(--card);
    color: var(--ink);
    font: 800 12.5px / 1.4 var(--font-body);
  }

  .plus {
    color: var(--ink-3);
    font-weight: 700;
  }
</style>
