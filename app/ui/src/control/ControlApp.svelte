<script lang="ts">
  import { tick } from "svelte";
  import * as api from "../lib/api";
  import { providePending, provideSession } from "../lib/context";
  import { createPendingChanges } from "../lib/pending";
  import type { Session } from "../lib/events";
  import { modelsAllReady } from "../lib/format";
  import Icon from "../lib/ui/Icon.svelte";
  import type { IconName } from "../lib/ui/icons";
  import Mascot from "../lib/ui/Mascot.svelte";
  import { homeStatus } from "../lib/wording";
  import FirstRun from "./FirstRun.svelte";
  import Notices from "./Notices.svelte";
  import About from "./pages/About.svelte";
  import Captions from "./pages/Captions.svelte";
  import History from "./pages/History.svelte";
  import Home from "./pages/Home.svelte";
  import Listening from "./pages/Listening.svelte";
  import Screenshot from "./pages/Screenshot.svelte";
  import Shortcuts from "./pages/Shortcuts.svelte";
  import Translation from "./pages/Translation.svelte";

  let { session }: { session: Session } = $props();
  // The session is fixed for the lifetime of the window.
  // svelte-ignore state_referenced_locally
  provideSession(session);
  // svelte-ignore state_referenced_locally
  const pending = createPendingChanges(session);
  providePending(pending);
  $effect(() => () => pending.dispose());

  const PAGES = [
    { id: "home", label: "Home", icon: "home" },
    { id: "captions", label: "Captions", icon: "captions" },
    { id: "listening", label: "Listening", icon: "listening" },
    { id: "translation", label: "Translation", icon: "translation" },
    { id: "screenshot", label: "Screenshot", icon: "screenshot" },
    { id: "shortcuts", label: "Shortcuts", icon: "shortcuts" },
    { id: "history", label: "History", icon: "history" },
    { id: "about", label: "About & help", icon: "about" },
  ] as const satisfies readonly { id: string; label: string; icon: IconName }[];
  type PageId = (typeof PAGES)[number]["id"];

  // svelte-ignore state_referenced_locally
  const { config, appState, models, loadError } = session;

  let page = $state<PageId>("home");
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

  const mood = $derived($config ? homeStatus($appState, $config).mood : "ready");

  async function finishFirstRun(): Promise<void> {
    if (started) return;
    started = true;
    firstRun = false;
    page = $config?.translate.engine === "openai" ? "translation" : $config?.capture.mode === "apps" ? "listening" : "home";
    await session.act("run", () => api.startListening());
  }

  function go(next: string): void {
    if (PAGES.some((item) => item.id === next)) page = next as PageId;
  }

  async function onKey(event: KeyboardEvent, index: number): Promise<void> {
    const last = PAGES.length - 1;
    const keys: Record<string, number> = {
      ArrowDown: index === last ? 0 : index + 1,
      ArrowUp: index === 0 ? last : index - 1,
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
</script>

{#if !ready}
  <p class="loading" role="status">Getting ready…</p>
{:else if $loadError && !$config}
  <div class="loading" role="alert">
    <p>{$loadError}</p>
    <button type="button" class="field" onclick={() => location.reload()}>Try again</button>
  </div>
{:else if $config}
  {#if firstRun}
    <FirstRun onready={finishFirstRun} />
  {:else}
    <div class="shell">
      <nav class="nav" aria-label="Settings pages">
        <div class="brand"><Mascot state={mood} size={24} />Live Translation</div>
        <div class="tabs" role="tablist" aria-orientation="vertical" aria-label="Settings pages">
          {#each PAGES as item, index (item.id)}
            {#if item.id === "about"}<span class="nav-spacer"></span>{/if}
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
              <Icon name={item.icon} size={20} />{item.label}
            </button>
          {/each}
        </div>
      </nav>

      <main class="content">
        <Notices />
        <div id="page-panel" role="tabpanel" aria-labelledby={`tab-${page}`}>
          {#if page === "home"}
            <Home config={$config} onnavigate={go} />
          {:else if page === "captions"}
            <Captions config={$config} />
          {:else if page === "listening"}
            <Listening config={$config} />
          {:else if page === "translation"}
            <Translation config={$config} />
          {:else if page === "screenshot"}
            <Screenshot config={$config} onnavigate={go} />
          {:else if page === "shortcuts"}
            <Shortcuts config={$config} />
          {:else if page === "history"}
            <History config={$config} />
          {:else}
            <About config={$config} />
          {/if}
        </div>
      </main>
    </div>
  {/if}
{/if}

<style>
  .tabs {
    flex: 1;
  }
</style>
