<script lang="ts">
  import * as api from "../../lib/api";
  import { usePending, useSession } from "../../lib/context";
  import { TRANSLATOR_MODEL_ID, threadsPatch, type Config } from "../../lib/settings";
  import Button from "../../lib/ui/Button.svelte";
  import ChoiceCard from "../../lib/ui/ChoiceCard.svelte";
  import Note from "../../lib/ui/Note.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";
  import Switch from "../../lib/ui/Switch.svelte";
  import { ONLINE_SERVICES, backupNames, privacyText, serviceName, translatorKind, type Engine, type OnlineEngine } from "../../lib/wording";
  import ModelLine from "../ModelLine.svelte";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const pending = usePending();
  const { models, appState, busy } = session;

  const BUILT_IN_PROMPT = "Translate the user's Chinese text into natural English. Output only the translation.";

  let testing = $state(false);
  let testResult = $state("");
  // svelte-ignore state_referenced_locally
  let lastOnline = $state<OnlineEngine>(translatorKind(config.translate.engine) === "online" ? (config.translate.engine as OnlineEngine) : "microsoft");

  const engine = $derived(config.translate.engine);
  const kind = $derived(translatorKind(engine));
  const localModel = $derived($models?.find((model) => model.id === TRANSLATOR_MODEL_ID));
  const waitingForLocal = $derived($pending.some((change) => change.model === TRANSLATOR_MODEL_ID));
  const shownKind = $derived(waitingForLocal ? "local" : kind);
  const privacy = $derived(privacyText(config));
  const threadOptions = $derived(Array.from({ length: Math.max($appState?.logical_cpus ?? 1, config.translate.threads) }, (_, index) => index + 1));

  $effect(() => {
    if (kind === "online") lastOnline = engine as OnlineEngine;
  });

  const saveEngine = (next: Engine) => {
    testResult = "";
    return session.save("translate.engine", { translate: { engine: next } });
  };

  async function choose(next: "online" | "local" | "server"): Promise<void> {
    if (next !== "local") pending.cancel(TRANSLATOR_MODEL_ID);
    if (next === "online") await saveEngine(lastOnline);
    else if (next === "server") await saveEngine("openai");
    else await pending.request({ model: TRANSLATOR_MODEL_ID, key: "translate.engine", patch: { translate: { engine: "local" } } });
  }

  async function chooseService(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await saveEngine(select.value as Engine);
    if (!saved) select.value = engine;
  }

  async function saveOpenAi(event: Event, key: keyof Config["translate"]["openai"]): Promise<void> {
    const input = event.currentTarget as HTMLInputElement | HTMLTextAreaElement;
    testResult = "";
    const saved = await session.save(`translate.openai.${key}`, { translate: { openai: { [key]: input.value } } });
    if (!saved) input.value = config.translate.openai[key];
  }

  async function testTranslator(): Promise<void> {
    testing = true;
    testResult = "";
    try {
      const result = await api.testTranslator();
      testResult = `${result.text} (${result.ms} ms)`;
    } catch (error) {
      testResult = api.readableError(error);
    } finally {
      testing = false;
    }
  }

  async function chooseThreads(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("translate.threads", threadsPatch("translate", Number(select.value)));
    if (!saved) select.value = String(config.translate.threads);
  }
</script>

<section class="page" aria-labelledby="translation-title">
  <div class="page-head">
    <h2 id="translation-title">Translation</h2>
    <p>Who turns the Chinese text into English, for captions and screenshots.</p>
  </div>

  <Note tone={kind === "local" ? "sage" : kind === "server" ? "mist" : "lemon"} icon={kind === "local" ? "shield" : "info"} role="note" testid="privacy-note">
    <b>{privacy.headline}</b> {privacy.detail}
  </Note>

  <Section title="Translator">
    <div class="choices" role="radiogroup" aria-label="Translator">
      <ChoiceCard
        name="translator"
        value="online"
        icon="online"
        title="Free online service"
        description="Fast to set up, nothing to download. Needs internet."
        meta="Microsoft, Google, Bing, Youdao"
        checked={shownKind === "online"}
        testid="translator-online"
        onchange={() => choose("online")}
      />
      <ChoiceCard
        name="translator"
        value="local"
        icon="pc"
        title="On this PC"
        description="Private and works offline. Uses CPU and memory while it runs."
        meta={localModel?.state === "ready" ? "Downloaded" : "1.07 GB download"}
        checked={shownKind === "local"}
        testid="translator-local"
        onchange={() => choose("local")}
      />
      <ChoiceCard
        name="translator"
        value="server"
        icon="server"
        title="My own AI server"
        description="Any OpenAI-compatible server, such as LM Studio or a paid API."
        checked={shownKind === "server"}
        testid="translator-server"
        onchange={() => choose("server")}
      />
    </div>
  </Section>

  {#if shownKind === "online"}
    <Section title="Online service">
      <Row title="Service">
        {#snippet control()}
          <select class="field" aria-label="Service" data-testid="translate-engine" value={engine} onchange={chooseService}>
            {#each ONLINE_SERVICES as service (service.value)}<option value={service.value}>{service.label}</option>{/each}
          </select>
        {/snippet}
      </Row>
      <Row title={`If ${serviceName(engine)} doesn't answer, try ${backupNames(engine)}`} help="It tries once more first. The backup sees the same Chinese text.">
        {#snippet control()}
          <Switch
            label="Use a backup service"
            checked={config.translate.fallback}
            testid="translate-fallback"
            onchange={(fallback) => session.save("translate.fallback", { translate: { fallback } })}
          />
        {/snippet}
      </Row>
    </Section>
  {:else if shownKind === "local"}
    <Section title="On this PC">
      {#if localModel?.state === "ready"}
        <Row title="Tencent Hy-MT2 1.8B" help="Downloaded. It starts when listening starts or a screenshot needs it.">
          {#snippet control()}<ModelLine model={localModel} label="Translator" />{/snippet}
        </Row>
      {:else}
        <Row
          title="Downloading the translator"
          help={`Tencent Hy-MT2 1.8B, once. Until it finishes, captions keep using ${serviceName(engine)}.`}
          stacked
        >
          {#snippet control()}<ModelLine model={localModel} label="Translator" />{/snippet}
        </Row>
      {/if}
      <Row title="Give games priority" help="Games stay smooth. English may show up a bit later.">
        {#snippet control()}
          <Switch
            label="Give games priority"
            checked={config.performance.low_priority}
            onchange={(low_priority) => session.save("performance.low_priority", { performance: { low_priority } })}
          />
        {/snippet}
      </Row>
      <details class="advanced">
        <summary>Advanced</summary>
        <Row title="Translator threads">
          {#snippet control()}
            <select class="field" aria-label="Translator threads" data-testid="translate-threads" value={String(config.translate.threads)} onchange={chooseThreads}>
              {#each threadOptions as count (count)}<option value={String(count)}>{count}</option>{/each}
            </select>
          {/snippet}
        </Row>
      </details>
    </Section>
  {:else}
    <Section title="Your server">
      <Row title="Address" stacked>
        {#snippet control()}
          <input class="field wide" type="url" aria-label="Address" value={config.translate.openai.base_url} placeholder="http://localhost:1234/v1" onchange={(event) => saveOpenAi(event, "base_url")} />
        {/snippet}
      </Row>
      <Row title="Model name" stacked>
        {#snippet control()}
          <input class="field wide" aria-label="Model name" value={config.translate.openai.model} onchange={(event) => saveOpenAi(event, "model")} />
        {/snippet}
      </Row>
      <Row title="API key" help="Optional. Saved as plain text in config.toml on this PC." stacked>
        {#snippet control()}
          <input class="field wide" type="password" autocomplete="off" aria-label="API key" value={config.translate.openai.api_key} onchange={(event) => saveOpenAi(event, "api_key")} />
        {/snippet}
      </Row>
      <Row title="Instruction" help="Leave it empty to use the built-in one." stacked>
        {#snippet control()}
          <textarea class="field wide" rows="3" aria-label="Instruction" value={config.translate.openai.prompt} placeholder={BUILT_IN_PROMPT} onchange={(event) => saveOpenAi(event, "prompt")}></textarea>
        {/snippet}
      </Row>
      <Row title="Test it" help="Sends one Chinese sentence with the saved settings and shows the answer.">
        {#snippet control()}
          <Button size="small" busy={testing || $busy.size > 0} onclick={testTranslator}>{testing ? "Testing…" : "Test"}</Button>
        {/snippet}
      </Row>
      {#if testResult}
        <Row title={testResult} testid="translator-test-result" />
      {/if}
    </Section>
  {/if}
</section>
