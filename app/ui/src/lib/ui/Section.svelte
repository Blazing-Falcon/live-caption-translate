<script lang="ts">
  import type { Snippet } from "svelte";

  let { title = "", testid, children }: { title?: string; testid?: string; children: Snippet } = $props();
</script>

<section class="section" data-testid={testid}>
  {#if title}<h3>{title}</h3>{/if}
  {@render children()}
</section>

<style>
  .section {
    display: flex;
    flex-direction: column;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-card);
    background: var(--card);
    box-shadow: var(--sticker-shadow);
    min-width: 0;
  }

  h3 {
    margin: 0;
    padding: 14px 18px 4px;
    font: 600 17px / 1.25 var(--font-head);
  }
</style>
