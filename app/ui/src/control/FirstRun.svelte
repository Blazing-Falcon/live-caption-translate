<script lang="ts">
  import { get } from "svelte/store";
  import * as api from "../lib/api";
  import { useSession } from "../lib/context";
  import { formatBytes, modelTitle, modelsAllReady, modelsPercent } from "../lib/format";
  import { modelSourcePatch, type Config } from "../lib/settings";
  import type { ModelSource, ModelStatus } from "../lib/types";
  import Notices from "./Notices.svelte";

  interface Props {
    config: Config;
    onready: () => void;
  }

  let { config, onready }: Props = $props();

  const session = useSession();
  const { models, busy } = session;

  let fired = false;
  // Optional models that are not recommended for this PC download only when ticked.
  let ticked = $state<string[]>([]);

  const rows = $derived($models ?? []);
  const wanted = (row: ModelStatus): boolean => !row.optional || row.recommended || ticked.includes(row.id);
  const totalBytes = $derived(rows.filter(wanted).reduce((sum, row) => sum + row.bytes_total, 0));
  const downloading = $derived(rows.some((row) => row.state === "downloading"));
  const paused = $derived(rows.some((row) => row.state === "paused"));
  const needsDownload = $derived(rows.some((row) => row.state === "missing" || row.state === "corrupt"));
  const source = $derived<ModelSource>(config.models.source);

  const action = $derived(
    downloading ? "pause" : paused ? "resume" : needsDownload ? "download" : null,
  );

  $effect(() => {
    if (!fired && modelsAllReady($models)) {
      fired = true;
      onready();
    }
  });

  function statusText(row: ModelStatus): string {
    switch (row.state) {
      case "missing":
        return waitingText(row);
      case "downloading":
        return `${modelsPercent(row)}%`;
      case "paused":
        return `Paused · ${modelsPercent(row)}%`;
      case "verifying":
        return "Checking file…";
      case "ready":
        return "Done";
      case "corrupt":
        return "Damaged";
    }
  }

  function sizeText(row: ModelStatus): string {
    if (row.state === "downloading" || row.state === "paused") {
      return `${formatBytes(row.bytes_done)} / ${formatBytes(row.bytes_total)}`;
    }
    // The draft model's title already carries its size.
    return row.id === api.DRAFT_MODEL_ID ? "" : formatBytes(row.bytes_total);
  }

  function waitingText(row: ModelStatus): string {
    return row.optional && !wanted(row) ? "Optional" : "Waiting";
  }

  function toggleOptional(row: ModelStatus, checked: boolean): void {
    ticked = checked ? [...ticked.filter((id) => id !== row.id), row.id] : ticked.filter((id) => id !== row.id);
  }

  async function start(): Promise<void> {
    if (get(busy).has("models.download")) return;
    // Without ids the app downloads every missing required model and the recommended optional ones.
    // A ticked optional model needs the whole list spelled out.
    const picked = rows.filter((row) => row.optional && !row.recommended && ticked.includes(row.id));
    const ids =
      picked.length === 0
        ? undefined
        : rows.filter((row) => wanted(row) && row.state !== "ready").map((row) => row.id);
    await session.act("models.download", () => api.modelsDownload(source, ids));
  }

  async function pause(): Promise<void> {
    if (get(busy).has("models.pause")) return;
    await session.act("models.pause", () => api.modelsPause());
  }

  async function primary(): Promise<void> {
    if (action === "pause") await pause();
    else await start();
  }

  async function chooseSource(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const next: ModelSource = select.value === "modelscope" ? "modelscope" : "huggingface";
    const saved = await session.save("models.source", modelSourcePatch(next));
    if (!saved) select.value = config.models.source;
  }

  async function useExisting(): Promise<void> {
    const folder = await session.act("models.existing", () =>
      api.pickFolder("Choose the folder that has your model files"),
    );
    if (!folder) return;
    const updated = await session.act("models.existing", () => api.modelsUseExisting(folder));
    if (updated) session.setModels(updated);
  }
</script>

<main class="first-run" aria-labelledby="first-run-title">
  <Notices />
  <div class="intro">
    <h1 id="first-run-title">Download the speech and translation models</h1>
    <p>
      Everything runs on this PC. The models are downloaded once{totalBytes > 0 ? `, about ${formatBytes(totalBytes)},` : ","}
      and nothing you listen to leaves your computer.
    </p>
  </div>

  <ul class="models" aria-label="Models">
    {#each rows as row (row.id)}
      <li class="model" data-state={row.state}>
        <div class="grow">
          <div class="names">
            {#if row.optional && !row.recommended}
              <label class="optional">
                <input
                  type="checkbox"
                  aria-label={`Download ${modelTitle(row)}`}
                  checked={ticked.includes(row.id)}
                  disabled={row.state !== "missing"}
                  onchange={(event) => toggleOptional(row, event.currentTarget.checked)}
                />
                <span class="title">{modelTitle(row)}</span>
              </label>
            {:else}
              <span class="title">{modelTitle(row)}</span>
            {/if}
          </div>
          {#if row.state === "downloading" || row.state === "paused"}
            <div
              class="track"
              role="progressbar"
              aria-label={`${row.name} download`}
              aria-valuemin="0"
              aria-valuemax="100"
              aria-valuenow={modelsPercent(row)}
            >
              <span style:width={`${modelsPercent(row)}%`}></span>
            </div>
          {/if}
        </div>
        <span class="size mono">{sizeText(row)}</span>
        <span class="status" class:warning={row.state === "corrupt"}>{statusText(row)}</span>
        {#if row.state === "corrupt"}
          <button type="button" class="btn" aria-disabled={$busy.has("models.download")} onclick={start}>Retry</button>
        {/if}
      </li>
    {:else}
      <li class="model">Looking for the model list…</li>
    {/each}
  </ul>

  <div class="controls">
    <label class="source">
      <span>Download from</span>
      <select class="select" value={source} disabled={downloading} onchange={chooseSource}>
        <option value="huggingface">Hugging Face</option>
        <option value="modelscope">ModelScope (mainland China)</option>
      </select>
    </label>
    <button type="button" class="link" disabled={downloading} onclick={useExisting}>Use files I already have</button>
    <span class="spacer"></span>
    {#if action}
      <button
        type="button"
        class="btn"
        class:primary={action !== "pause"}
        aria-disabled={$busy.has("models.download") || $busy.has("models.pause")}
        onclick={primary}
      >
        {action === "pause" ? "Pause" : action === "resume" ? "Resume" : "Download"}
      </button>
    {/if}
  </div>
</main>

<style>
  .first-run {
    display: flex;
    flex-direction: column;
    gap: 28px;
    height: 100%;
    padding: 48px 64px;
    overflow: auto;
  }

  .intro {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 560px;
  }

  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 600;
  }

  p {
    margin: 0;
    font-size: 15px;
    line-height: 1.55;
    color: var(--c-text-label);
  }

  .models {
    display: flex;
    flex-direction: column;
    max-width: 640px;
    margin: 0;
    padding: 0;
    list-style: none;
    background: var(--c-surface);
    border: 1px solid var(--c-border);
    border-radius: var(--radius-card);
  }

  .model {
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: 14px var(--space-4);
    border-bottom: 1px solid var(--c-row-sep);
  }

  .model:last-child {
    border-bottom: 0;
  }

  .grow {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .title {
    font-weight: 500;
  }

  .optional {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
  }

  .size {
    font-size: var(--text-help);
    color: var(--c-text-secondary);
  }

  .status {
    min-width: 96px;
    font-size: var(--text-label);
    color: var(--c-text-label);
    text-align: right;
  }

  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-3) 20px;
    max-width: 640px;
  }

  .source {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-label);
    color: var(--c-text-label);
  }

  .spacer {
    flex: 1;
  }

  .link:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
