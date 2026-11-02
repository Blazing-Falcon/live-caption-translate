<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { overlayPatch, type Config } from "../../lib/settings";
  import Button from "../../lib/ui/Button.svelte";
  import Icon from "../../lib/ui/Icon.svelte";
  import Keys from "../../lib/ui/Keys.svelte";
  import Mascot from "../../lib/ui/Mascot.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";
  import Switch from "../../lib/ui/Switch.svelte";
  import Tag from "../../lib/ui/Tag.svelte";
  import { healthRows, homeStatus, translatorKind, translatorLabel } from "../../lib/wording";

  let { config, onnavigate }: { config: Config; onnavigate: (page: string) => void } = $props();

  const session = useSession();
  const { appState, stats, busy } = session;

  const status = $derived(homeStatus($appState, config));
  const listening = $derived($appState?.listening ?? "starting");
  const visible = $derived($appState?.overlay_visible ?? config.overlay.visible);
  const moving = $derived($appState?.overlay_moving ?? false);
  const local = $derived(config.translate.engine === "local");
  const health = $derived(healthRows($stats, listening === "listening", local));
  const source = $derived(
    config.capture.mode === "apps"
      ? config.capture.apps.map((app) => app.name).join(", ") || "No apps picked"
      : $appState?.source?.label.trim() || "Whole PC",
  );

  async function toggleRun(): Promise<void> {
    if (get(busy).has("run") || listening === "starting") return;
    await session.act("run", () => (listening === "listening" ? api.pauseListening() : api.startListening()));
  }
</script>

<section class="page" aria-labelledby="home-title">
  <div class="hero" data-testid="home-status" data-mood={status.mood}>
    <Mascot state={status.mood} size={62} />
    <div class="words">
      <h2 id="home-title" data-testid="header-title">{status.title}</h2>
      <p data-testid="header-sub">{status.detail}</p>
      <div class="chips">
        <button type="button" class="chip" onclick={() => onnavigate("listening")}><Icon name="listening" size={14} />{source}</button>
        <button type="button" class="chip" onclick={() => onnavigate("translation")}>
          <Icon name={translatorKind(config.translate.engine) === "online" ? "online" : local ? "pc" : "server"} size={14} />{translatorLabel(config.translate.engine)}
        </button>
        {#if config.asr.second_pass}<Tag tone="sage">Double-check on</Tag>{/if}
      </div>
    </div>
    <Button
      variant={listening === "paused" ? "primary" : "normal"}
      size="big"
      busy={$busy.has("run") || listening === "starting"}
      onclick={toggleRun}
    >
      {listening === "listening" ? "Pause" : listening === "paused" ? "Start" : "Starting…"}
    </Button>
  </div>

  <div class="two">
    <Section title="Quick switches">
      <Row title="Show captions">
        {#snippet control()}
          <Switch
            label="Show captions"
            checked={visible}
            onchange={(on) => session.act("overlay.visible", () => api.setOverlayVisible(on))}
          />
        {/snippet}
      </Row>
      <Row title="Chinese above the English">
        {#snippet control()}
          <Switch
            label="Chinese above the English"
            checked={config.overlay.show_source}
            onchange={(on) => session.save("overlay.show_source", overlayPatch({ show_source: on }))}
          />
        {/snippet}
      </Row>
      <Row title="Move captions">
        {#snippet help()}Drag them, then press <Keys combo={config.hotkeys.move_lock || "Ctrl+Shift+L"} /> or Lock.{/snippet}
        {#snippet control()}
          <Button size="small" busy={$busy.has("overlay.moving")} onclick={() => session.act("overlay.moving", () => api.setOverlayMoving(!moving))}>
            {moving ? "Lock" : "Move"}
          </Button>
        {/snippet}
      </Row>
    </Section>

    <Section title="How it's going" testid="health">
      {#each health as row (row.label)}
        <div class="stat"><span>{row.label}</span><b data-tone={row.tone}>{row.value}</b></div>
      {:else}
        <Row title="Numbers show while listening." help="How soon English appears, whether it keeps up, and how much CPU it uses." />
      {/each}
    </Section>
  </div>

  <Section title="Translate something on screen">
    <Row help="Works in games, apps and videos, even while captions are paused.">
      {#snippet title()}
        {#if config.hotkeys.screen_translate}Press <Keys combo={config.hotkeys.screen_translate} />, then drag a box around Chinese text.{:else}Set a shortcut to translate text on the screen.{/if}
      {/snippet}
      {#snippet control()}<Button size="small" onclick={() => onnavigate("screenshot")}>Screenshot settings</Button>{/snippet}
    </Row>
  </Section>
</section>

<style>
  .hero {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 18px;
    padding: 20px 22px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-card);
    background: var(--card);
    box-shadow: var(--sticker-shadow);
  }

  .hero[data-mood="trouble"] {
    background: var(--hl-rose);
  }

  .words {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  h2 {
    margin: 0;
    font: 600 26px / 1.2 var(--font-head);
  }

  p {
    margin: 0;
    color: var(--ink-2);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 10px;
    border: 1.5px solid var(--line);
    border-radius: var(--r-pill);
    background: var(--paper-2);
    color: var(--ink-2);
    font: 800 12.5px / 1.6 var(--font-body);
    cursor: pointer;
  }

  .chip:hover {
    border-color: var(--stroke);
    color: var(--ink);
  }

  .two {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 16px;
    align-items: start;
  }

  .stat {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 2px 12px;
    padding: 10px 18px;
    border-top: 1.5px dashed var(--line);
  }

  .stat span {
    color: var(--ink-2);
  }

  .stat b {
    margin-left: auto;
    text-align: right;
  }

  .stat b[data-tone="good"] {
    color: var(--good);
  }

  .stat b[data-tone="warn"] {
    color: var(--warn);
  }
</style>
