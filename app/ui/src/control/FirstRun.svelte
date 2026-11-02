<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../lib/api";
  import { useSession } from "../lib/context";
  import { formatBytes, modelsAllReady, modelsPercent } from "../lib/format";
  import { SECOND_PASS_MODEL_ID, SPEECH_MODEL_ID, TRANSLATOR_MODEL_ID, type Config } from "../lib/settings";
  import type { ModelStatus } from "../lib/types";
  import Button from "../lib/ui/Button.svelte";
  import ChoiceCard from "../lib/ui/ChoiceCard.svelte";
  import Mascot from "../lib/ui/Mascot.svelte";
  import Progress from "../lib/ui/Progress.svelte";
  import Tag from "../lib/ui/Tag.svelte";
  import { translatorKind, type TranslatorKind } from "../lib/wording";
  import Notices from "./Notices.svelte";

  let { onready }: { onready: () => void } = $props();

  const session = useSession();
  const { models, busy, config } = session;

  const initial = get(config);
  let step = $state(0);
  let translator = $state<TranslatorKind>(initial && translatorKind(initial.translate.engine) !== "local" ? translatorKind(initial.translate.engine) : "online");
  let secondPass = $state(initial?.asr.second_pass ?? true);
  let source = $state<Config["capture"]["mode"]>(initial?.capture.mode ?? "system");
  let fired = false;

  const size = (id: string): number => $models?.find((model) => model.id === id)?.bytes_total ?? 0;
  const total = $derived(size(SPEECH_MODEL_ID) + (secondPass ? size(SECOND_PASS_MODEL_ID) : 0) + (translator === "local" ? size(TRANSLATOR_MODEL_ID) : 0));
  const needed = $derived(($models ?? []).filter((model) => !model.optional));
  const done = $derived(needed.reduce((sum, model) => sum + (model.state === "ready" ? model.bytes_total : model.bytes_done), 0));
  const neededTotal = $derived(needed.reduce((sum, model) => sum + model.bytes_total, 0));
  const downloading = $derived(needed.some((model) => model.state === "downloading" || model.state === "verifying"));
  const paused = $derived(needed.some((model) => model.state === "paused"));
  const corrupt = $derived(needed.some((model) => model.state === "corrupt"));

  $effect(() => {
    if (!fired && step === 3 && modelsAllReady($models)) {
      fired = true;
      onready();
    }
  });

  function engineFor(kind: TranslatorKind): Config["translate"]["engine"] {
    const current = get(config)?.translate.engine ?? "microsoft";
    if (kind === "local") return "local";
    if (kind === "server") return "openai";
    return translatorKind(current) === "online" ? current : "microsoft";
  }

  async function start(): Promise<void> {
    const saved = await session.save("first-run", {
      translate: { engine: engineFor(translator) },
      asr: { second_pass: secondPass },
      capture: { mode: source },
    });
    if (!saved) return;
    await session.refreshModels();
    step = 3;
    await session.act("models.download", () => api.modelsDownload());
  }

  async function pauseOrResume(): Promise<void> {
    if (downloading) await session.act("models.pause", () => api.modelsPause());
    else await session.act("models.download", () => api.modelsDownload());
  }

  async function useExisting(): Promise<void> {
    const folder = await session.act("models.existing", () => api.pickFolder("Choose the folder that has your model files"));
    if (!folder) return;
    const updated = await session.act("models.existing", () => api.modelsUseExisting(folder));
    if (updated) session.setModels(updated);
  }

  function rowState(model: ModelStatus): string {
    switch (model.state) {
      case "ready":
        return "Done";
      case "downloading":
        return `Downloading · ${modelsPercent(model)}%`;
      case "paused":
        return `Paused · ${modelsPercent(model)}%`;
      case "verifying":
        return "Checking the file…";
      case "corrupt":
        return "Damaged";
      default:
        return "Waiting";
    }
  }
</script>

<main class="first-run" aria-labelledby="first-run-title">
  <Notices />
  <div class="steps" aria-hidden="true">
    {#each [0, 1, 2, 3] as index (index)}<i class:on={index <= step}></i>{/each}
  </div>

  {#if step === 0}
    <div class="hello">
      <Mascot state="ready" size={70} />
      <div>
        <h1 id="first-run-title">Hi! Let's set up your captions.</h1>
        <p>Three quick choices. You can change all of them later.</p>
      </div>
    </div>
    <h2>How should Chinese be translated?</h2>
    <div class="choices" role="radiogroup" aria-label="Translator">
      <ChoiceCard name="wizard-translator" value="online" icon="online" title="Free online service" description="Start in a minute. Only the Chinese text goes to Microsoft; audio stays on this PC." meta="Nothing extra to download" checked={translator === "online"} onchange={() => (translator = "online")} />
      <ChoiceCard name="wizard-translator" value="local" icon="pc" title="On this PC" description="Private and offline. Needs a bigger download and some CPU." meta={`+${formatBytes(size(TRANSLATOR_MODEL_ID))}`} checked={translator === "local"} onchange={() => (translator = "local")} />
      <ChoiceCard name="wizard-translator" value="server" icon="server" title="My own AI server" description="You enter its address on the Translation page after this." meta="Nothing extra to download" checked={translator === "server"} onchange={() => (translator = "server")} />
    </div>
    <div class="nav-row"><span></span><Button variant="primary" size="big" onclick={() => (step = 1)}>Next</Button></div>
  {:else if step === 1}
    <h1 id="first-run-title">Double-check what you hear?</h1>
    <p>After each line, the app can listen again with the words around it and fix mistakes. It's more accurate and uses more CPU.</p>
    <div class="choices" role="radiogroup" aria-label="Double-check">
      <ChoiceCard name="wizard-second" value="yes" icon="check" title="Yes, double-check" description="Recommended for drama, interviews and fast talk." meta={`+${formatBytes(size(SECOND_PASS_MODEL_ID))}`} checked={secondPass} onchange={() => (secondPass = true)} />
      <ChoiceCard name="wizard-second" value="no" icon="listening" title="Not now" description="Faster and lighter. You can turn it on later on the Listening page." checked={!secondPass} onchange={() => (secondPass = false)} />
    </div>
    <div class="nav-row"><Button onclick={() => (step = 0)}>Back</Button><Button variant="primary" size="big" onclick={() => (step = 2)}>Next</Button></div>
  {:else if step === 2}
    <h1 id="first-run-title">What should we listen to?</h1>
    <div class="choices" role="radiogroup" aria-label="Listen to">
      <ChoiceCard name="wizard-source" value="system" icon="listening" title="Everything on this PC" description="The simplest. Captions everything you hear." checked={source === "system"} onchange={() => (source = "system")} />
      <ChoiceCard name="wizard-source" value="apps" icon="apps" title="Only the apps I pick" description="You pick them on the Listening page. Needs Windows 11." checked={source === "apps"} onchange={() => (source = "apps")} />
    </div>
    <div class="total" data-testid="download-total">
      <span><b>To download:</b> speech{secondPass ? ", double-check" : ""}{translator === "local" ? ", translator" : ""}</span>
      <b class="size">{formatBytes(total)}</b>
    </div>
    <div class="nav-row">
      <Button onclick={() => (step = 1)}>Back</Button>
      <Button variant="primary" size="big" busy={$busy.has("first-run")} onclick={start}>Download and start</Button>
    </div>
  {:else}
    <div class="hello">
      <Mascot state={corrupt ? "trouble" : "ready"} size={70} />
      <div>
        <h1 id="first-run-title">Getting your captions ready</h1>
        <p>{formatBytes(done)} of {formatBytes(neededTotal)}. You can keep using your PC; captions start by themselves.</p>
      </div>
    </div>
    <Progress percent={neededTotal > 0 ? (done / neededTotal) * 100 : 0} label="Download" />
    <ul class="models" aria-label="Models">
      {#each needed as model (model.id)}
        <li class="model" data-state={model.state}>
          <span class="name">{model.name}</span>
          <span class="size">{formatBytes(model.bytes_total)}</span>
          <Tag tone={model.state === "ready" ? "sage" : model.state === "corrupt" ? "rose" : model.state === "missing" ? "plain" : "lemon"}>{rowState(model)}</Tag>
        </li>
      {/each}
    </ul>
    <div class="nav-row">
      <Button variant="quiet" busy={downloading} onclick={useExisting}>Use files I already have</Button>
      {#if downloading || paused || corrupt}
        <Button busy={$busy.has("models.download") || $busy.has("models.pause")} onclick={pauseOrResume}>
          {downloading ? "Pause" : corrupt ? "Retry" : "Resume"}
        </Button>
      {/if}
    </div>
  {/if}
</main>

<style>
  .first-run {
    display: flex;
    flex-direction: column;
    gap: 18px;
    max-width: 720px;
    margin: 0 auto;
    padding: 30px 34px;
  }

  .steps {
    display: flex;
    gap: 6px;
  }

  .steps i {
    flex: 1;
    height: 8px;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-pill);
    background: var(--paper-2);
  }

  .steps i.on {
    background: var(--pen);
  }

  .hello {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  h1 {
    margin: 0;
    font: 600 28px / 1.2 var(--font-head);
  }

  h2 {
    margin: 0;
    font: 600 20px / 1.25 var(--font-head);
  }

  p {
    margin: 0;
    color: var(--ink-2);
  }

  .choices {
    display: grid;
    gap: 10px;
  }

  .nav-row {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 10px;
  }

  .total {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border: var(--stroke-w) dashed var(--stroke);
    border-radius: var(--r-ctl);
    background: var(--paper-2);
  }

  .size {
    font-variant-numeric: tabular-nums;
  }

  .total .size {
    font-size: 18px;
  }

  .models {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-card);
    background: var(--card);
  }

  .model {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border-top: 1.5px dashed var(--line);
  }

  .model:first-child {
    border-top: 0;
  }

  .model .size {
    color: var(--ink-3);
    font-size: 13px;
  }
</style>
