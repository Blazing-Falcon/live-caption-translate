<script lang="ts" module>
  /** A speech-bubble doodle whose face shows the app's state. Never shown over video. */
  export type MascotState = "listening" | "ready" | "paused" | "trouble";
</script>

<script lang="ts">
  let { state = "listening", size = 56 }: { state?: MascotState; size?: number } = $props();
</script>

<svg
  class="mascot"
  data-state={state}
  width={size * 1.25}
  height={size}
  viewBox="0 0 80 64"
  aria-hidden="true"
>
  <path
    class="bubble"
    d="M12 6 h42 a9 9 0 0 1 9 9 v22 a9 9 0 0 1 -9 9 h-24 l-11 10 v-10 h-7 a9 9 0 0 1 -9 -9 v-22 a9 9 0 0 1 9 -9z"
  />
  {#if state === "paused"}
    <path class="line" d="M21 26 q4 3 8 0 M37 26 q4 3 8 0" />
  {:else if state === "trouble"}
    <circle class="ink" cx="25" cy="26" r="3" />
    <circle class="ink" cx="41" cy="26" r="3" />
    <path class="line" d="M20 19 l7 2 M46 19 l-7 2" />
  {:else}
    <circle class="ink" cx="25" cy={state === "ready" ? 23 : 25} r="3.2" />
    <circle class="ink" cx="41" cy={state === "ready" ? 23 : 25} r="3.2" />
  {/if}
  <ellipse class="cheek" cx="18.5" cy="32" rx="3.4" ry="2.2" />
  <ellipse class="cheek" cx="47.5" cy="32" rx="3.4" ry="2.2" />
  <path
    class="line"
    d={state === "listening"
      ? "M29 32 q4 4 8 0"
      : state === "paused"
        ? "M30 33 h6"
        : state === "ready"
          ? "M30 32 q3 3 6 0"
          : "M29 34 q4 -3 8 0"}
  />
  {#if state === "listening"}
    <path class="wave" d="M66 12 q5 6 0 12 M71 8 q8 10 0 20" />
  {:else if state === "paused"}
    <text class="zz" x="60" y="15" font-size="13">z</text>
    <text class="zz" x="68" y="8" font-size="9">z</text>
  {:else if state === "ready"}
    <circle class="dot" cx="64" cy="10" r="2" />
    <circle class="dot faint" cx="70" cy="10" r="2" />
    <circle class="dot fainter" cx="76" cy="10" r="2" />
  {:else}
    <path class="drop" d="M64 6 q-4 7 0 9 q4 -2 0 -9z" />
  {/if}
</svg>

<style>
  .mascot {
    flex: none;
    overflow: visible;
  }

  .bubble {
    fill: var(--card);
    stroke: var(--ink);
    stroke-width: 2.4;
    stroke-linejoin: round;
  }

  .mascot[data-state="paused"] .bubble {
    fill: var(--paper-2);
  }

  .mascot[data-state="trouble"] .bubble {
    fill: var(--hl-rose);
  }

  .ink {
    fill: var(--ink);
  }

  .line,
  .wave {
    fill: none;
    stroke: var(--ink);
    stroke-width: 2.2;
    stroke-linecap: round;
  }

  .wave {
    stroke: var(--pen-text);
    stroke-width: 2.4;
  }

  .cheek {
    fill: var(--cheek);
    opacity: 0.8;
  }

  .zz {
    fill: var(--ink-3);
    font-family: var(--font-head);
    font-weight: 700;
  }

  .dot {
    fill: var(--pen-text);
  }

  .faint {
    opacity: 0.6;
  }

  .fainter {
    opacity: 0.3;
  }

  .drop {
    fill: var(--hl-mist);
    stroke: var(--pen-text);
    stroke-width: 1.6;
  }
</style>
