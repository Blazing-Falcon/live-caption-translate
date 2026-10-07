<script lang="ts">
  import { tick } from "svelte";
  import * as api from "../lib/api";
  import { provideSession } from "../lib/context";
  import type { Session } from "../lib/events";
  import { footerText, modelsAllReady } from "../lib/format";
  import FirstRun from "./FirstRun.svelte";
  import HeaderCard from "./HeaderCard.svelte";
  import Notices from "./Notices.svelte";
  import Capture from "./pages/Capture.svelte";
  import Languages from "./pages/Languages.svelte";
  import Overlay from "./pages/Overlay.svelte";
  import Performance from "./pages/Performance.svelte";

  let { session }: { session: Session } = $props();
  // The session is fixed for the lifetime of the window.
  // svelte-ignore state_referenced_locally
  provideSession(session);

  const PAGES = [
    { id: "capture", label: "Capture" },
    { id: "overlay", label: "Overlay" },
    { id: "languages", label: "Languages" },
    { id: "performance", label: "Performance" },
  ] as const;
  type PageId = (typeof PAGES)[number]["id"];

  // svelte-ignore state_referenced_locally
  const { config, appState, stats, models, loadError } = session;

  let page = $state<PageId>("capture");
  let ready = $state(false);
  let firstRun = $state(false);
  let started = false;

  // svelte-ignore state_referenced_locally
  session.ready.then(() => {
    ready = true;
  });

  $effect(() => {
    if (ready && !firstRun && !started && $models !== null && !modelsAllReady($models)) firstRun = true;
  });

  async function finishFirstRun(): Promise<void> {
    if (started) return;
    started = true;
    firstRun = false;
    page = "capture";
    await session.act("run", () => api.startListening());
  }

  async function onKey(event: KeyboardEvent, index: number): Promise<void> {
    const last = PAGES.length - 1;
    const keys: Record<string, number> = {
      ArrowDown: index === last ? 0 : index + 1,
      ArrowRight: index === last ? 0 : index + 1,
      ArrowUp: index === 0 ? last : index - 1,
      ArrowLeft: index === 0 ? last : index - 1,
      Home: 0,
      End: last,
    };
    const target = keys[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = PAGES[target];
    if (!next) return;
    page = next.id;
    await tick();
    document.getElementById(`tab-${next.id}`)?.focus();
  }

  const footer = $derived(footerText($appState?.listening, $stats));
</script>

{#if !ready}
  <p class="loading" role="status">Starting…</p>
{:else if $loadError && !$config}
  <div class="loading" role="alert">
    <p>{$loadError}</p>
    <button type="button" class="btn" onclick={() => location.reload()}>Try again</button>
  </div>
{:else if $config}
  {#if firstRun}
    <FirstRun config={$config} onready={finishFirstRun} />
  {:else}
    <div class="shell">
      <nav class="nav" aria-label="Settings pages">
        <div class="tabs" role="tablist" aria-orientation="vertical" aria-label="Settings pages">
          {#each PAGES as item, index (item.id)}
            <button
              type="button"
              role="tab"
              class="nav-item"
              id={`tab-${item.id}`}
              aria-selected={page === item.id}
              aria-controls="page-panel"
              tabindex={page === item.id ? 0 : -1}
              onclick={() => (page = item.id)}
              onkeydown={(event) => onKey(event, index)}
            >
              {item.label}
            </button>
          {/each}
        </div>
      </nav>

      <main class="content">
        <Notices />
        <HeaderCard />
        <div class="page-panel" id="page-panel" role="tabpanel" aria-labelledby={`tab-${page}`}>
          {#if page === "capture"}
            <Capture config={$config} />
          {:else if page === "overlay"}
            <Overlay config={$config} />
          {:else if page === "languages"}
            <Languages config={$config} />
          {:else}
            <Performance config={$config} />
          {/if}
        </div>
      </main>

      <footer class="footer" data-testid="footer">
        <span>{footer}</span>
      </footer>
    </div>
  {/if}
{/if}

<style>
  .loading {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-7);
    color: var(--c-text-secondary);
  }
</style>
