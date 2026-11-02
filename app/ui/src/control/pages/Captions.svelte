<script lang="ts">
  import * as api from "../../lib/api";
  import { CHINESE_PRESETS, ENGLISH_PRESETS, nightCardWarning } from "../../lib/colors";
  import { useSession } from "../../lib/context";
  import { DEFAULT_CONFIG, LOW_BACKGROUND_WARNING, OVERLAY_LIMITS, overlayPatch, type Config } from "../../lib/settings";
  import Button from "../../lib/ui/Button.svelte";
  import ColorPicker from "../../lib/ui/ColorPicker.svelte";
  import Keys from "../../lib/ui/Keys.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";
  import Segmented from "../../lib/ui/Segmented.svelte";
  import Slider from "../../lib/ui/Slider.svelte";
  import Switch from "../../lib/ui/Switch.svelte";
  import CaptionPreview from "../CaptionPreview.svelte";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { appState, busy } = session;

  type Overlay = Config["overlay"];
  let fontDraft = $state<number | null>(null);
  let seeThroughDraft = $state<number | null>(null);
  let scene = $state<"dark" | "bright" | "game">("dark");

  const overlay = $derived(config.overlay);
  const seeThrough = $derived(seeThroughDraft ?? Math.round((1 - overlay.background) * 100));
  /** What the preview shows while a slider is held, before it is saved. */
  const shown = $derived<Overlay>({
    ...overlay,
    font_px: fontDraft ?? overlay.font_px,
    background: seeThroughDraft === null ? overlay.background : 1 - seeThroughDraft / 100,
  });
  const night = $derived(overlay.card === "night");
  const englishWarning = $derived(night ? nightCardWarning(overlay.english_color) : null);
  const chineseWarning = $derived(night ? nightCardWarning(overlay.chinese_color) : null);
  const moving = $derived($appState?.overlay_moving ?? false);
  const linesKey = $derived(overlay.style === "panel" ? "panel_lines" : "flow_lines");
  const lineCounts = Array.from({ length: OVERLAY_LIMITS.panel_lines.max }, (_, index) => index + 1);

  const LAYOUT_HELP: Record<Overlay["style"], string> = {
    panel: "Lines stack on one side of the screen.",
    flow: "One English paragraph that keeps growing.",
    subtitles: "The newest line at the bottom, like film subtitles, with the one before it above.",
  };

  const save = (key: string, values: Partial<Overlay>) => session.save(`overlay.${key}`, overlayPatch(values));

  async function saveLines(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await save(linesKey, { [linesKey]: Number(select.value) });
    if (!saved) select.value = String(overlay[linesKey]);
  }

  const resetColors = () =>
    save("colors", { english_color: DEFAULT_CONFIG.overlay.english_color, chinese_color: DEFAULT_CONFIG.overlay.chinese_color });
</script>

<section class="page" aria-labelledby="captions-title">
  <div class="page-head">
    <h2 id="captions-title">Captions</h2>
    <p>How English captions look on top of your video or game.</p>
  </div>

  <Section>
    <div class="preview-head">
      <b>Preview</b>
      <Segmented
        name="preview-scene"
        label="Preview background"
        value={scene}
        options={[
          { value: "dark", label: "Dark video" },
          { value: "bright", label: "Bright video" },
          { value: "game", label: "Game" },
        ]}
        onchange={(value) => (scene = value)}
      />
    </div>
    <div class="preview"><CaptionPreview overlay={shown} {scene} /></div>
  </Section>

  <Section title="Look">
    <Row title="Layout" help={LAYOUT_HELP[overlay.style]}>
      {#snippet control()}
        <Segmented
          name="style"
          label="Layout"
          value={overlay.style}
          options={[
            { value: "panel", label: "Side panel" },
            { value: "flow", label: "Running text" },
            { value: "subtitles", label: "Film subtitles" },
          ]}
          onchange={(style) => save("style", { style })}
        />
      {/snippet}
    </Row>
    <Row title="Card" help="Night is a dark card that works on any video. Outline has no card, like film subtitles.">
      {#snippet control()}
        <Segmented
          name="card"
          label="Card"
          value={overlay.card}
          options={[
            { value: "night", label: "Night" },
            { value: "outline", label: "Outline" },
          ]}
          onchange={(card) => save("card", { card })}
        />
      {/snippet}
    </Row>
    <Row title="Font" help={overlay.font === "clear" ? "Clear is made for low vision." : ""}>
      {#snippet control()}
        <Segmented
          name="font"
          label="Font"
          value={overlay.font}
          options={[
            { value: "rounded", label: "Rounded" },
            { value: "clear", label: "Clear" },
            { value: "system", label: "System" },
          ]}
          onchange={(font) => save("font", { font })}
        />
      {/snippet}
    </Row>
  </Section>

  <Section title="Colors">
    <Row title="English" help="The translation." stacked testid="english-color">
      {#snippet control()}
        <ColorPicker name="english-color" label="English color" presets={ENGLISH_PRESETS} value={overlay.english_color} onchange={(english_color) => save("english_color", { english_color })} />
        {#if englishWarning}<span class="warning-text" role="note">{englishWarning}</span>{/if}
      {/snippet}
    </Row>
    <Row title="Chinese" help="The finished line above it, and words while someone is talking." stacked testid="chinese-color">
      {#snippet control()}
        <ColorPicker name="chinese-color" label="Chinese color" presets={CHINESE_PRESETS} value={overlay.chinese_color} onchange={(chinese_color) => save("chinese_color", { chinese_color })} />
        {#if chineseWarning}<span class="warning-text" role="note">{chineseWarning}</span>{/if}
      {/snippet}
    </Row>
    <Row title="Back to the default colors">
      {#snippet control()}<Button variant="quiet" size="small" onclick={resetColors}>Reset colors</Button>{/snippet}
    </Row>
  </Section>

  <Section title="Text">
    <Row title="English size" stacked>
      {#snippet control()}
        <div class="grow">
          <Slider
            id="font-size"
            label="English size"
            min={OVERLAY_LIMITS.font_px.min}
            max={OVERLAY_LIMITS.font_px.max}
            value={fontDraft ?? overlay.font_px}
            format={(value) => `${value} px`}
            oninput={(value) => (fontDraft = value)}
            onchange={async (font_px) => {
              await save("font_px", { font_px });
              fontDraft = null;
            }}
          />
        </div>
      {/snippet}
    </Row>
    {#if night}
      <Row title="Card see-through" stacked help={seeThrough / 100 >= 1 - LOW_BACKGROUND_WARNING ? "" : "Higher lets more of the video show through."}>
        {#snippet control()}
          <div class="grow">
            <Slider
              id="see-through"
              label="Card see-through"
              min={0}
              max={100}
              value={seeThrough}
              format={(value) => `${value}%`}
              oninput={(value) => (seeThroughDraft = value)}
              onchange={async (value) => {
                await save("background", { background: Math.round(100 - value) / 100 });
                seeThroughDraft = null;
              }}
            />
          </div>
          {#if (1 - seeThrough / 100) <= LOW_BACKGROUND_WARNING}
            <span class="warning-text" role="note" data-testid="background-warning">More than 50% see-through can be hard to read on bright video.</span>
          {/if}
        {/snippet}
      </Row>
    {/if}
    <Row title="Chinese above the English" help="The finished Chinese line, smaller, above each English line.">
      {#snippet control()}
        <Switch label="Chinese above the English" checked={overlay.show_source} onchange={(show_source) => save("show_source", { show_source })} />
      {/snippet}
    </Row>
    <Row title="Chinese while someone is talking" help="Shows words as they're heard, before the line is finished.">
      {#snippet control()}
        <Switch label="Chinese while someone is talking" checked={overlay.live_source} onchange={(live_source) => save("live_source", { live_source })} />
      {/snippet}
    </Row>
{#if overlay.style !== "subtitles"}
          <Row title={overlay.style === "panel" ? "Lines kept on screen" : "Lines of running text"} help="Older lines roll off the top.">
      {#snippet control()}
        <select class="field" aria-label={overlay.style === "panel" ? "Lines kept on screen" : "Lines of running text"} value={String(overlay[linesKey])} onchange={saveLines}>
          {#each lineCounts as count (count)}<option value={String(count)}>{count}</option>{/each}
        </select>
      {/snippet}
    </Row>
    {/if}
  </Section>

  <Section title="Position">
    {#if overlay.style === "panel"}
      <Row title="Side of the screen">
        {#snippet control()}
          <Segmented
            name="edge"
            label="Side of the screen"
            value={overlay.panel_edge}
            options={[
              { value: "left", label: "Left" },
              { value: "right", label: "Right" },
            ]}
            onchange={(panel_edge) => save("panel_edge", { panel_edge })}
          />
        {/snippet}
      </Row>
    {/if}
    <Row title="Move captions">
      {#snippet help()}
        {#if config.hotkeys.move_lock}Or press <Keys combo={config.hotkeys.move_lock} /> anywhere.{:else}Drag them where you want, then lock.{/if}
      {/snippet}
      {#snippet control()}
        <Button size="small" busy={$busy.has("overlay.moving")} aria-pressed={moving} onclick={() => session.act("overlay.moving", () => api.setOverlayMoving(!moving))}>
          {moving ? "Lock" : "Move"}
        </Button>
      {/snippet}
    </Row>
  </Section>
</section>

<style>
  .preview-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 14px 18px 8px;
  }

  .preview {
    padding: 0 18px 18px;
  }

  .grow {
    width: 100%;
  }
</style>
