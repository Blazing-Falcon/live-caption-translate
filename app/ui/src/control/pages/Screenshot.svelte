<script lang="ts">
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { OCR_MODEL_ID } from "../../lib/screen";
  import type { Config } from "../../lib/settings";
  import Button from "../../lib/ui/Button.svelte";
  import Keys from "../../lib/ui/Keys.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";
  import Segmented from "../../lib/ui/Segmented.svelte";
  import { translatorLabel } from "../../lib/wording";
  import ModelLine from "../ModelLine.svelte";

  let { config, onnavigate }: { config: Config; onnavigate: (page: string) => void } = $props();

  const session = useSession();
  const { models, busy } = session;

  const reader = $derived($models?.find((model) => model.id === OCR_MODEL_ID));
  const canDownload = $derived(reader !== undefined && (reader.state === "missing" || reader.state === "paused" || reader.state === "corrupt"));
  const action = $derived(reader?.state === "paused" ? "Resume" : reader?.state === "corrupt" ? "Retry" : "Download");

  async function download(): Promise<void> {
    if ($busy.has("models.screen")) return;
    await session.act("models.screen", () => api.modelsDownloadOptional(OCR_MODEL_ID));
  }
</script>

<section class="page" aria-labelledby="screenshot-title">
  <div class="page-head">
    <h2 id="screenshot-title">Screenshot</h2>
    <p>Freeze the screen and translate the Chinese text in a box you drag.</p>
  </div>

  <Section title="How to use it">
    <Row help="The screen under your mouse freezes.">
      {#snippet title()}
        {#if config.hotkeys.screen_translate}Press <Keys combo={config.hotkeys.screen_translate} />{:else}The shortcut is off{/if}
      {/snippet}
      {#snippet control()}<Button size="small" onclick={() => onnavigate("shortcuts")}>Change</Button>{/snippet}
    </Row>
    <Row title="Drag a box around Chinese text" help="English is painted over it. Hold “Hold for original” to peek at the Chinese." />
    <Row>
      {#snippet title()}Press <Keys combo="Esc" /> or right-click to close{/snippet}
      {#snippet help()}<Keys combo="D" /> shows the boxes the reader found.{/snippet}
    </Row>
  </Section>

  <Section title="Reading">
    <Row title="Text direction" help="Down is for top-to-bottom text, like some menus and manga.">
      {#snippet control()}
        <Segmented
          name="direction"
          label="Text direction"
          value={config.screen.direction}
          options={[
            { value: "horizontal", label: "Across" },
            { value: "vertical", label: "Down" },
          ]}
          onchange={(direction) => session.save("screen.direction", { screen: { direction } })}
        />
      {/snippet}
    </Row>
    <Row title="Text reader" help="Reads the text in screenshots. Used only for this, and listening never needs it." testid="screen-model">
      {#snippet control()}
        <ModelLine model={reader} label="Text reader" />
        {#if canDownload}<Button size="small" busy={$busy.has("models.screen")} onclick={download}>{action}</Button>{/if}
      {/snippet}
    </Row>
  </Section>

  <p class="small-print">Screenshots use the same translator as captions ({translatorLabel(config.translate.engine)}). Images never leave this PC.</p>
</section>
