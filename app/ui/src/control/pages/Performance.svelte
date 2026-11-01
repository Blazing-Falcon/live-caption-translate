<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { reconcileChecked } from "../../lib/reconcile";
  import {
    formatBytes,
    formatMemory,
    formatSeconds,
    modelsPercent,
    totalCpu,
    totalMemoryMb,
  } from "../../lib/format";
  import { OCR_MODEL_ID, modelBusy } from "../../lib/screen";
  import {
    asrPatch,
    formatAccelerator,
    performancePatch,
    threadsPatch,
    transcriptPatch,
    type Config,
  } from "../../lib/settings";
  import type { ModelState } from "../../lib/types";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { stats, busy, appState, models, config: liveConfig } = session;

  const SCREEN_MODEL_STATE: Record<ModelState, string> = {
    missing: "Not downloaded",
    downloading: "Downloading",
    paused: "Paused",
    verifying: "Checking file…",
    ready: "Ready",
    corrupt: "Damaged",
  };
  const SCREEN_MODEL_ACTION: Partial<Record<ModelState, string>> = {
    missing: "Download",
    paused: "Resume",
    corrupt: "Retry",
  };

  const screenModel = $derived($models?.find((row) => row.id === OCR_MODEL_ID) ?? null);
  const screenShortcut = $derived(formatAccelerator(config.hotkeys.screen_translate));

  // Up to the PC's logical CPUs; a larger saved value stays selectable.
  const threadOptions = (current: number): number[] =>
    Array.from({ length: Math.max($appState?.logical_cpus ?? 1, current) }, (_, index) => index + 1);

  async function toggleLowPriority(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("performance.low_priority", performancePatch({ low_priority: input.checked }));
    reconcileChecked(input, get(liveConfig)?.performance.low_priority ?? config.performance.low_priority);
  }

  let testing = $state(false);
  let testResult = $state("");
  const builtInPrompt = "Translate the user's Chinese text into natural English. Output only the translation.";

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

  async function chooseEngine(event: Event): Promise<void> {
    testResult = "";
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("translate.engine", { translate: { engine: select.value as Config["translate"]["engine"] } });
    if (!saved) select.value = config.translate.engine;
  }

  async function chooseThreads(event: Event, table: "asr" | "translate"): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save(`${table}.threads`, threadsPatch(table, Number(select.value)));
    if (!saved) select.value = String(config[table].threads);
  }

  async function chooseSecondPassThreads(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("asr.second_pass_threads", asrPatch({ second_pass_threads: Number(select.value) }));
    if (!saved) select.value = String(config.asr.second_pass_threads);
  }

  async function toggleTranscript(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("transcript.enabled", transcriptPatch(input.checked));
    reconcileChecked(input, get(liveConfig)?.transcript.enabled ?? config.transcript.enabled);
  }

  async function downloadScreenModel(): Promise<void> {
    if (get(busy).has("models.screen")) return;
    await session.act("models.screen", () => api.modelsDownloadOptional(OCR_MODEL_ID));
  }

  const openTranscripts = async (): Promise<void> => {
    if (get(busy).has("open.transcripts")) return;
    await session.act("open.transcripts", () => api.openFolder("transcripts"));
  };
</script>

<section class="page" aria-labelledby="performance-title">
  <h2 id="performance-title" class="section-title">Performance</h2>

  <div class="stack">
    <h3 class="section-title">Speech recognition</h3>
    <label class="field">
      <span>Recognizer threads</span>
      <select
        class="select"
        value={String(config.asr.threads)}
        data-testid="asr-threads"
        onchange={(event) => chooseThreads(event, "asr")}
      >
        {#each threadOptions(config.asr.threads) as count (count)}
          <option value={String(count)}>{count}</option>
        {/each}
      </select>
    </label>
    <label class="field">
      <span>Correction threads</span>
      <select
        class="select"
        value={String(config.asr.second_pass_threads)}
        data-testid="asr-second-pass-threads"
        onchange={chooseSecondPassThreads}
      >
        {#each threadOptions(config.asr.second_pass_threads) as count (count)}
          <option value={String(count)}>{count}</option>
        {/each}
      </select>
    </label>
    <p class="help small">
      Each finished line is read again by X-ASR offline with the speech before it, and the corrected Chinese replaces the live text before translation.
    </p>
  </div>

  <div class="stack">
    <h3 class="section-title">Translation</h3>
    <label class="field">
      <span>Translator</span>
      <select class="select" value={config.translate.engine} data-testid="translate-engine" onchange={chooseEngine}>
        <option value="local">Local model</option>
        <option value="microsoft">Microsoft</option>
        <option value="google">Google</option>
        <option value="google_beta">Google Beta</option>
        <option value="openai">OpenAI-compatible</option>
      </select>
    </label>
    {#if config.translate.engine !== "local"}
      <p class="help small">Captions and screenshot text are sent to {config.translate.engine === "openai" ? "your configured endpoint" : config.translate.engine === "microsoft" ? "Microsoft" : config.translate.engine === "google_beta" ? "Google Beta" : "Google"} to be translated.</p>
    {/if}
    {#if config.translate.engine === "openai"}
      <label class="field">
        <span>Base URL</span>
        <input class="input" type="url" value={config.translate.openai.base_url} placeholder="http://localhost:1234/v1" onchange={(event) => saveOpenAi(event, "base_url")} />
      </label>
      <label class="field">
        <span>Model</span>
        <input class="input" value={config.translate.openai.model} onchange={(event) => saveOpenAi(event, "model")} />
      </label>
      <label class="field">
        <span>API key</span>
        <input class="input" type="password" autocomplete="off" value={config.translate.openai.api_key} onchange={(event) => saveOpenAi(event, "api_key")} />
      </label>
      <p class="help small">The API key is stored in plain text in config.toml.</p>
      <label class="field">
        <span>Translation prompt</span>
        <textarea class="input" rows="3" value={config.translate.openai.prompt} placeholder={builtInPrompt} onchange={(event) => saveOpenAi(event, "prompt")}></textarea>
      </label>
      <button class="button" disabled={testing || $busy.size > 0} onclick={testTranslator}>{testing ? "Testing…" : "Test translator"}</button>
      {#if testResult}<p class="help small" role="status" data-testid="translator-test-result">{testResult}</p>{/if}
    {/if}
    <div class="field">
      <span>Local translation model</span>
      <span class="model-name" data-testid="translate-model">Tencent Hy-MT2 1.8B (1.25-bit)</span>
    </div>
    <label class="field">
      <span>Local model threads</span>
      <select
        class="select"
        value={String(config.translate.threads)}
        data-testid="translate-threads"
        onchange={(event) => chooseThreads(event, "translate")}
      >
        {#each threadOptions(config.translate.threads) as count (count)}
          <option value={String(count)}>{count}</option>
        {/each}
      </select>
    </label>
    <div class="option">
      <label class="check-row">
        <input
          type="checkbox"
          checked={config.performance.low_priority}
          aria-describedby="help-low-priority"
          onchange={toggleLowPriority}
        />
        <span>Give games priority</span>
      </label>
      <p id="help-low-priority" class="help small">
        Runs the local translation model at low priority so games stay smooth. Off: English can appear slightly sooner, but uses more CPU.
      </p>
    </div>
  </div>

  <div class="stack">
    <h3 class="section-title">Screen translation</h3>
    {#if screenModel}
      <div class="model-row" data-testid="screen-model" data-state={screenModel.state}>
        <div class="model-main">
          <span>Text recognition model</span>
          {#if modelBusy(screenModel) || screenModel.state === "paused"}
            <progress max="100" value={modelsPercent(screenModel)} aria-label="Text recognition model download"></progress>
          {/if}
        </div>
        <span class="size mono">{formatBytes(screenModel.bytes_total)}</span>
        <span class="state" class:warning={screenModel.state === "corrupt"}>
          {SCREEN_MODEL_STATE[screenModel.state]}{screenModel.state === "downloading" || screenModel.state === "paused"
            ? ` · ${modelsPercent(screenModel)}%`
            : ""}
        </span>
        {#if SCREEN_MODEL_ACTION[screenModel.state]}
          <button type="button" class="btn" aria-disabled={$busy.has("models.screen")} onclick={downloadScreenModel}>
            {SCREEN_MODEL_ACTION[screenModel.state]}
          </button>
        {/if}
      </div>
    {/if}
    <p class="help small">
      {screenShortcut
        ? `Press ${screenShortcut} to freeze the screen, then drag a box over Chinese text to read and translate it.`
        : "The screen translation shortcut is off. Set one on the Overlay page."}
      The model downloads once and is only used for this. Listening does not need it.
    </p>
  </div>

  <dl class="stat-grid">
    <div class="stat">
      <dt>Word to English</dt>
      <dd data-testid="stat-final">{formatSeconds($stats?.word_final_p50_ms)}</dd>
      <span class="note">median, last minute</span>
    </div>
    <div class="stat">
      <dt>Waiting to translate</dt>
      <dd data-testid="stat-queue">{$stats ? $stats.queue_depth : "—"}</dd>
      <span class="note">skipped this session: {$stats ? $stats.skipped_total : "—"}</span>
    </div>
    <div class="stat">
      <dt>Memory</dt>
      <dd data-testid="stat-memory">{$stats ? formatMemory(totalMemoryMb($stats)) : "—"}</dd>
      <span class="note">
        CPU {$stats ? `${totalCpu($stats)}%` : "—"}, {config.translate.engine === "local" ? "app and translation server" : "app"}
      </span>
    </div>
  </dl>

  <div class="stack">
    <label class="check-row">
      <input
        type="checkbox"
        checked={config.transcript.enabled}
        onchange={toggleTranscript}
      />
      <span>Save a transcript of each listening session</span>
    </label>
    <div>
      <button type="button" class="btn" aria-disabled={$busy.has("open.transcripts")} onclick={openTranscripts}>
        Open transcript folder
      </button>
    </div>
  </div>
</section>

<style>
  .option {
    display: flex;
    flex-direction: column;
  }

  .option .check-row + .help {
    padding-left: 26px;
  }

  .model-name {
    font-size: var(--text-ui);
    color: var(--c-text);
  }

  .model-row {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    max-width: 520px;
    padding: var(--space-3) var(--space-4);
    background: var(--c-surface);
    border: 1px solid var(--c-border);
    border-radius: var(--radius-card);
  }

  .model-main {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .model-main progress {
    width: 100%;
    accent-color: var(--c-accent);
  }

  .size {
    font-size: var(--text-help);
    color: var(--c-text-secondary);
  }

  .state {
    min-width: 96px;
    font-size: var(--text-label);
    color: var(--c-text-label);
    text-align: right;
  }
</style>
