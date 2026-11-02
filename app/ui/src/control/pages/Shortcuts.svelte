<script lang="ts">
  import { tick } from "svelte";
  import { useSession } from "../../lib/context";
  import { HOTKEY_ACTIONS, hotkeyPatch, recordShortcut, type Config, type HotkeyAction } from "../../lib/settings";
  import Button from "../../lib/ui/Button.svelte";
  import Keys from "../../lib/ui/Keys.svelte";
  import Note from "../../lib/ui/Note.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { hotkeyErrors } = session;

  const LABELS: Record<HotkeyAction, string> = {
    move_lock: "Move or lock captions",
    show_hide: "Show or hide captions",
    pause: "Pause or start listening",
    settings: "Open this window",
    screen_translate: "Translate the screen",
  };
  const HELP: Partial<Record<HotkeyAction, string>> = {
    settings: "Captions let clicks pass through, so this is the way back here from a game.",
  };

  let recording = $state<HotkeyAction | null>(null);

  async function start(action: HotkeyAction): Promise<void> {
    recording = action;
    await tick();
    document.getElementById(`record-${action}`)?.focus();
  }

  async function save(action: HotkeyAction, accelerator: string): Promise<void> {
    recording = null;
    const saved = await session.save(`hotkeys.${action}`, hotkeyPatch(action, accelerator));
    if (saved) session.clearHotkeyError(action);
  }

  function onKey(event: KeyboardEvent, action: HotkeyAction): void {
    if (event.key === "Tab") {
      recording = null;
      return;
    }
    event.preventDefault();
    const pressed = recordShortcut(event);
    if (pressed.kind === "cancel") recording = null;
    else if (pressed.kind === "clear") void save(action, "");
    else if (pressed.kind === "shortcut") void save(action, pressed.accelerator);
  }
</script>

<section class="page" aria-labelledby="shortcuts-title">
  <div class="page-head">
    <h2 id="shortcuts-title">Shortcuts</h2>
    <p>They work everywhere, even in fullscreen games.</p>
  </div>

  <Section>
    {#each HOTKEY_ACTIONS as action (action)}
      {@const failure = $hotkeyErrors[action]}
      <Row title={LABELS[action]} testid={`hotkey-${action}`}>
        {#snippet help()}
          {#if failure}
            <span class="error-text" role="alert">{failure.accelerator || "This shortcut"} could not be used. {failure.message}</span>
          {:else}{HELP[action] ?? ""}{/if}
        {/snippet}
        {#snippet control()}
          {#if recording === action}
            <button
              type="button"
              class="recorder"
              id={`record-${action}`}
              aria-label={`Press the new keys for ${LABELS[action]}. Escape cancels, Backspace turns it off.`}
              onkeydown={(event) => onKey(event, action)}
              onblur={() => (recording = null)}
            >
              Press the new keys…
            </button>
          {:else}
            {#if config.hotkeys[action]}<Keys combo={config.hotkeys[action]} />{:else}<span class="off">Off</span>{/if}
            <Button variant="quiet" size="small" aria-label={`Change ${LABELS[action]}`} onclick={() => start(action)}>Change</Button>
          {/if}
        {/snippet}
      </Row>
    {/each}
  </Section>

  <Note tone="mist">Press <b>Change</b>, then the new keys. <b>Escape</b> cancels and <b>Backspace</b> turns a shortcut off. Ctrl+Alt+A is taken by QQ's screenshot, so pick something else for that one.</Note>
</section>

<style>
  .recorder {
    min-height: 32px;
    padding: 4px 14px;
    border: var(--stroke-w) dashed var(--pen-text);
    border-radius: var(--r-pill);
    background: var(--pen-soft);
    color: var(--ink);
    font: 700 13.5px / 1.2 var(--font-body);
  }

  .off {
    color: var(--ink-3);
    font-weight: 700;
  }
</style>
