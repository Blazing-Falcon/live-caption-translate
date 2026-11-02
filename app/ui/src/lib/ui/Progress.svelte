<script lang="ts">
  let { percent, label }: { percent: number; label: string } = $props();

  const width = $derived(Math.max(0, Math.min(100, percent)));
</script>

<div class="track" role="progressbar" aria-label={label} aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(width)}>
  <span style:width="{width}%"></span>
</div>

<style>
  .track {
    height: 12px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-pill);
    background: var(--paper-2);
    overflow: hidden;
  }

  span {
    display: block;
    height: 100%;
    background: repeating-linear-gradient(-45deg, var(--pen) 0 6px, var(--pen-soft) 6px 12px);
    transition: width 200ms linear;
  }
</style>
