<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { OCR_MODEL_ID } from "../../lib/screen";
  import { SECOND_PASS_MODEL_ID, SPEECH_MODEL_ID, TRANSLATOR_MODEL_ID, type Config } from "../../lib/settings";
  import Button from "../../lib/ui/Button.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";
  import Segmented from "../../lib/ui/Segmented.svelte";
  import ModelLine from "../ModelLine.svelte";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { models, busy } = session;

  const DOWNLOADS = [
    { id: SPEECH_MODEL_ID, label: "Speech", help: "Always needed." },
    { id: SECOND_PASS_MODEL_ID, label: "Double-check", help: "Needed when Double-check each line is on." },
    { id: TRANSLATOR_MODEL_ID, label: "Translator on this PC", help: "Needed when you translate on this PC." },
    { id: OCR_MODEL_ID, label: "Text reader", help: "Needed for screenshots." },
  ];
  const LEVELS: { value: Config["logging"]["level"]; label: string }[] = [
    { value: "warn", label: "Only problems" },
    { value: "info", label: "Normal" },
    { value: "debug", label: "Detailed" },
    { value: "trace", label: "Everything" },
  ];

  let version = $state("");
  let confirmReset = $state(false);

  onMount(() => {
    import("@tauri-apps/api/app")
      .then(({ getVersion }) => getVersion())
      .then((value) => (version = value ?? ""))
      .catch(() => undefined);
  });

  async function useExisting(): Promise<void> {
    const folder = await session.act("models.existing", () => api.pickFolder("Choose the folder that has your model files"));
    if (!folder) return;
    const updated = await session.act("models.existing", () => api.modelsUseExisting(folder));
    if (updated) session.setModels(updated);
  }

  async function chooseLevel(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("logging.level", { logging: { level: select.value as Config["logging"]["level"] } });
    if (!saved) select.value = config.logging.level;
  }

  async function reset(): Promise<void> {
    if ($busy.has("config.reset")) return;
    if (await session.reset()) confirmReset = false;
  }
</script>

<section class="page" aria-labelledby="about-title">
  <div class="page-head">
    <h2 id="about-title">About &amp; help</h2>
    <p>Live Translation{version ? ` ${version}` : ""}</p>
  </div>

  <Section title="Look">
    <Row title="App theme" help="Captions keep their own card.">
      {#snippet control()}
        <Segmented
          name="app-theme"
          label="App theme"
          value={config.app.theme}
          options={[
            { value: "system", label: "Like Windows" },
            { value: "light", label: "Light" },
            { value: "dark", label: "Dark" },
          ]}
          onchange={(theme) => session.save("app.theme", { app: { theme } })}
        />
      {/snippet}
    </Row>
  </Section>

  <Section title="Downloads">
    {#each DOWNLOADS as item (item.id)}
      {@const model = $models?.find((row) => row.id === item.id)}
      {#if model}
        <Row title={item.label} help={item.help}>
          {#snippet control()}<ModelLine {model} label={item.label} />{/snippet}
        </Row>
      {/if}
    {/each}
    <Row title="Download folder" help={config.models.dir || "The app's own folder."}>
      {#snippet control()}
        <Button size="small" busy={$busy.has("open.models")} onclick={() => session.act("open.models", () => api.openFolder("models"))}>Open folder</Button>
        <Button variant="quiet" size="small" busy={$busy.has("models.existing")} onclick={useExisting}>Use files I have</Button>
      {/snippet}
    </Row>
  </Section>

  <Section title="Troubleshooting">
    <Row title="Log detail" help="Raise it only when someone asks you for logs. Takes effect when the app starts again.">
      {#snippet control()}
        <select class="field" aria-label="Log detail" value={config.logging.level} onchange={chooseLevel}>
          {#each LEVELS as level (level.value)}<option value={level.value}>{level.label}</option>{/each}
          {#if !LEVELS.some((level) => level.value === config.logging.level)}<option value={config.logging.level}>{config.logging.level}</option>{/if}
        </select>
      {/snippet}
    </Row>
    <Row title="Log files">
      {#snippet control()}
        <Button size="small" busy={$busy.has("open.logs")} onclick={() => session.act("open.logs", () => api.openFolder("logs"))}>Open folder</Button>
      {/snippet}
    </Row>
    {#if confirmReset}
      <Row title="Reset all settings?" help="Your caption position, chosen apps and downloads stay." id="reset" stacked>
        {#snippet control()}
          <Button variant="danger" size="small" busy={$busy.has("config.reset")} aria-describedby="reset-help" data-testid="reset-confirm" onclick={reset}>Reset</Button>
          <Button variant="quiet" size="small" onclick={() => (confirmReset = false)}>Cancel</Button>
        {/snippet}
      </Row>
    {:else}
      <Row title="Reset settings">
        {#snippet control()}<Button size="small" data-testid="reset-settings" onclick={() => (confirmReset = true)}>Reset…</Button>{/snippet}
      </Row>
    {/if}
  </Section>
</section>
