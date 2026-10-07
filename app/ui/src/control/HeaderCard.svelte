<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../lib/api";
  import { useSession } from "../lib/context";

  const session = useSession();
  const { appState, config, busy } = session;

  const listening = $derived($appState?.listening ?? "starting");
  const target = $derived.by(() => {
    const label = $appState?.source?.label.trim();
    if (label) return label;
    if ($config?.capture.mode === "apps") {
      const names = $config.capture.apps.map((app) => app.name);
      return names.length > 0 ? names.join(", ") : "no apps picked";
    }
    return "whole system";
  });
  const title = $derived(listening === "listening" ? "Listening" : listening === "paused" ? "Paused" : "Starting");
  const sub = $derived(
    listening === "paused" ? "Captions are off. Models stay loaded." : `Chinese to English · ${target}`,
  );
  const working = $derived($busy.has("run"));

  async function toggle(): Promise<void> {
    if (get(busy).has("run") || listening === "starting") return;
    await session.act("run", () => (listening === "listening" ? api.pauseListening() : api.startListening()));
  }
</script>

<section class="card header-card" aria-label="Status" data-testid="header-card">
  <div class="who">
    <span class="dot" class:hollow={listening !== "listening"} aria-hidden="true"></span>
    <div class="text">
      <span class="title" data-testid="header-title">{title}</span>
      <span class="sub" data-testid="header-sub">{sub}</span>
    </div>
  </div>
  <button
    type="button"
    class="btn"
    class:primary={listening === "paused"}
    aria-disabled={working || listening === "starting"}
    onclick={toggle}
  >
    {listening === "listening" ? "Pause" : listening === "paused" ? "Start" : "Starting…"}
  </button>
</section>

<style>
  .header-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
  }

  .who {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }

  .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .title {
    font-size: 15px;
    font-weight: 500;
  }

  .sub {
    font-size: var(--text-label);
    color: var(--c-text-secondary);
    overflow-wrap: anywhere;
  }

  .dot {
    flex: none;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--c-accent);
  }

  .dot.hollow {
    background: transparent;
    border: 1.5px solid var(--c-hollow);
  }
</style>
