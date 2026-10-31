<script lang="ts">
  import { cssSize, frameUrl, type Screen } from "../lib/screen";
  import ResultLayer from "./ResultLayer.svelte";
  import Selection from "./Selection.svelte";
  import DebugBoxes from "./DebugBoxes.svelte";
  import Toolbar from "./Toolbar.svelte";

  let { screen }: { screen: Screen } = $props();

  // The controller is fixed for the lifetime of the window.
  // svelte-ignore state_referenced_locally
  const { state: view } = screen;

  let hint = $state<string | null>(null);

  // A new job starts with a clean status line.
  $effect(() => {
    void $view.selection;
    hint = null;
  });

  const size = $derived($view.frame ? cssSize($view.frame) : null);

  function keydown(event: KeyboardEvent): void {
    if (event.key === "Escape") screen.close();
    else if (event.target instanceof HTMLSelectElement) return;
    else if (event.key.toLowerCase() === "d" && !event.repeat && !event.ctrlKey && !event.altKey && !event.metaKey && $view.frame) {
      event.preventDefault();
      screen.toggleDebug();
    }
    else if (event.key === " " && $view.frame) {
      event.preventDefault();
      screen.setOriginal(true);
    }
  }

  function keyup(event: KeyboardEvent): void {
    if (event.key === " ") screen.setOriginal(false);
  }

  // A hidden window forgets its frame, so the next time it shows it never flashes the old one.
  function visibility(): void {
    if (document.visibilityState === "hidden") screen.reset();
  }
</script>

<svelte:window
  onkeydown={keydown}
  onkeyup={keyup}
  onblur={() => screen.setOriginal(false)}
  oncontextmenu={(event) => event.preventDefault()}
/>
<svelte:document onvisibilitychange={visibility} />

{#if $view.frame && size}
  <main class="screen" style:width="{size.w}px" style:height="{size.h}px">
    <img class="frame" src={frameUrl($view.frame.id)} alt="" draggable="false" />
    <Selection
      frame={$view.frame}
      selection={$view.selection}
      onselect={(rect) => screen.select(rect)}
      onclose={() => screen.close()}
    />
    <ResultLayer frame={$view.frame} job={$view.job} selection={$view.selection} blocks={$view.blocks} outcomes={$view.outcomes} hidden={$view.original} onhint={(message) => (hint = message)} />
    {#if $view.debug && !$view.original}
      <DebugBoxes frame={$view.frame} blocks={$view.blocks} lines={$view.ocrLines} />
    {/if}
      <Toolbar
        view={$view}
        {hint}
        viewport={size}
        oncopy={(kind) => screen.copy(kind)}
        ondownload={() => void screen.downloadModel()}
        onoriginal={(on) => screen.setOriginal(on)}
        onclose={() => screen.close()}
        ondirection={direction => void screen.setDirection(direction)}
      />
  </main>
{/if}

<style>
  .screen {
    position: relative;
    overflow: hidden;
  }

  .frame {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }

</style>
