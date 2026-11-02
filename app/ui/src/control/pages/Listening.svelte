<script lang="ts">
  import { onMount } from "svelte";
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { usePending, useSession } from "../../lib/context";
  import { levelText } from "../../lib/format";
  import {
    SECOND_PASS_MODEL_ID,
    asrPatch,
    captureAppsPatch,
    captureDevicePatch,
    captureModePatch,
    hasApp,
    threadsPatch,
    toggledApps,
    type CaptureApp,
    type Config,
  } from "../../lib/settings";
  import type { AudioApp, AudioAppList, AudioDevice } from "../../lib/types";
  import ChoiceCard from "../../lib/ui/ChoiceCard.svelte";
  import Row from "../../lib/ui/Row.svelte";
  import Section from "../../lib/ui/Section.svelte";
  import Slider from "../../lib/ui/Slider.svelte";
  import Switch from "../../lib/ui/Switch.svelte";
  import ModelLine from "../ModelLine.svelte";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const pending = usePending();
  const { config: liveConfig, appState, models } = session;

  let devices = $state<AudioDevice[]>([]);
  let appList = $state<AudioAppList | null>(null);
  let appError = $state<string | null>(null);
  let pauseDraft = $state<number | null>(null);

  const mode = $derived(config.capture.mode);
  const unsupported = $derived(appList !== null && !appList.supported);
  const reason = $derived(appList?.reason ?? "Picking apps needs Windows 11.");
  const defaultDevice = $derived(devices.find((device) => device.is_default));
  const deviceKnown = $derived(config.capture.device === "default" || devices.some((device) => device.id === config.capture.device));
  const secondModel = $derived($models?.find((model) => model.id === SECOND_PASS_MODEL_ID));
  const waitingForModel = $derived($pending.some((change) => change.model === SECOND_PASS_MODEL_ID));
  const secondOn = $derived(config.asr.second_pass || waitingForModel);
  const threadOptions = (current: number): number[] =>
    Array.from({ length: Math.max($appState?.logical_cpus ?? 1, current) }, (_, index) => index + 1);

  interface AppRow {
    exe: string;
    name: string;
    icon: string | null;
    level: string;
  }

  const rows = $derived.by<AppRow[]>(() => {
    const live: AppRow[] = (appList?.apps ?? []).map((app: AudioApp) => ({ exe: app.exe, name: app.name, icon: app.icon_png, level: levelText(app) }));
    const remembered: AppRow[] = config.capture.apps
      .filter((saved) => !live.some((row) => row.exe.toLowerCase() === saved.exe.toLowerCase()))
      .map((saved) => ({ exe: saved.exe, name: saved.name, icon: null, level: "not playing" }));
    return [...live, ...remembered];
  });

  async function loadApps(): Promise<void> {
    try {
      appList = await api.listAudioApps();
      appError = null;
    } catch (error) {
      appError = api.readableError(error);
    }
  }

  onMount(() => {
    api.listAudioDevices().then((list) => (devices = list)).catch((error: unknown) => session.pushNotice("error", api.readableError(error)));
    void loadApps();
  });

  $effect(() => {
    if (mode !== "apps") return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async (): Promise<void> => {
      if (stopped) return;
      if (!document.hidden) await loadApps();
      if (!stopped) timer = setTimeout(tick, 1000);
    };
    timer = setTimeout(tick, 1000);
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  });

  async function chooseDevice(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("capture.device", captureDevicePatch(select.value));
    if (!saved) select.value = config.capture.device;
  }

  // The patch replaces the whole list, so each toggle starts from the list the previous one saved.
  let appSaves: Promise<void> = Promise.resolve();

  function toggleApp(event: Event, row: AppRow): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const entry: CaptureApp = { exe: row.exe, name: row.name };
    const checked = input.checked;
    appSaves = appSaves.then(async () => {
      const current = get(liveConfig)?.capture.apps ?? config.capture.apps;
      await session.save("capture.apps", captureAppsPatch(toggledApps(current, entry, checked)));
      input.checked = hasApp(get(liveConfig)?.capture.apps ?? [], row.exe);
    });
    return appSaves;
  }

  async function setSecondPass(on: boolean): Promise<void> {
    if (!on) {
      pending.cancel(SECOND_PASS_MODEL_ID);
      await session.save("asr.second_pass", asrPatch({ second_pass: false }));
      return;
    }
    await pending.request({ model: SECOND_PASS_MODEL_ID, key: "asr.second_pass", patch: asrPatch({ second_pass: true }) });
  }

  async function chooseThreads(event: Event, key: "threads" | "second_pass_threads"): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const value = Number(select.value);
    const saved = await session.save(`asr.${key}`, key === "threads" ? threadsPatch("asr", value) : asrPatch({ second_pass_threads: value }));
    if (!saved) select.value = String(config.asr[key]);
  }
</script>

<section class="page" aria-labelledby="listening-title">
  <div class="page-head">
    <h2 id="listening-title">Listening</h2>
    <p>What the app hears and how it turns speech into Chinese text.</p>
  </div>

  <Section title="Listen to">
    <div class="choices" role="radiogroup" aria-label="Listen to">
      <ChoiceCard
        name="source"
        value="system"
        icon="listening"
        title="Everything on this PC"
        description="All sound from one speaker or headset."
        checked={mode === "system"}
        onchange={() => session.save("capture.mode", captureModePatch("system"))}
      />
      <ChoiceCard
        name="source"
        value="apps"
        icon="apps"
        title="Only the apps I pick"
        description="Leave out music, calls and game sounds you don't need. Needs Windows 11."
        checked={mode === "apps"}
        disabled={unsupported}
        onchange={() => session.save("capture.mode", captureModePatch("apps"))}
      />
    </div>
    {#if unsupported}
      <Row title={reason} help={mode === "apps" ? "Listening to everything for now. Your picks are saved." : ""} testid="apps-unsupported" />
    {/if}
    {#if mode === "system" || unsupported}
      <Row title="Speaker or headset">
        {#snippet control()}
          <select class="field" aria-label="Speaker or headset" value={config.capture.device} onchange={chooseDevice}>
            <option value="default">{defaultDevice ? `Same as Windows (${defaultDevice.name})` : "Same as Windows"}</option>
            {#each devices as device (device.id)}<option value={device.id}>{device.name}</option>{/each}
            {#if !deviceKnown}<option value={config.capture.device}>Saved device (not connected)</option>{/if}
          </select>
        {/snippet}
      </Row>
    {:else}
      <Row title="Playing sound now" help="Picked apps reconnect when they restart." />
      <div class="apps" role="group" aria-label="Apps playing sound now">
        {#each rows as row (row.exe.toLowerCase())}
          <label class="app">
            <input type="checkbox" checked={hasApp(config.capture.apps, row.exe)} onchange={(event) => toggleApp(event, row)} />
            {#if row.icon}<img class="icon" alt="" src={`data:image/png;base64,${row.icon}`} />{:else}<span class="icon"></span>{/if}
            <span class="name">{row.name}</span>
            <span class="level">{row.level}</span>
          </label>
        {:else}
          <p class="small-print empty">No apps are playing sound right now.</p>
        {/each}
        {#if appError}<p class="error-text empty">{appError}</p>{/if}
      </div>
    {/if}
    <Row title="Start listening when the app opens">
      {#snippet control()}
        <Switch
          label="Start listening when the app opens"
          checked={config.capture.autostart}
          onchange={(autostart) => session.save("capture.autostart", { capture: { autostart } })}
        />
      {/snippet}
    </Row>
  </Section>

  <Section title="Understanding speech">
    <Row
      title="Double-check each line"
      help="When a line ends, listen to it again with the words around it and fix what was misheard. Uses more CPU and can add up to 1.5 s before the line is translated."
    >
      {#snippet control()}
        <Switch label="Double-check each line" checked={secondOn} onchange={setSecondPass} testid="second-pass" />
      {/snippet}
    </Row>
    {#if secondOn && secondModel}
      <Row
        title={secondModel.state === "ready" ? "Double-check model" : "Double-check needs a one-time download"}
        help={waitingForModel ? "Captions keep working without it. It turns on when the download finishes." : ""}
      >
        {#snippet control()}<ModelLine model={secondModel} label="Double-check model" />{/snippet}
      </Row>
    {/if}
    <Row title="Start a new line after a pause of" help="Lines also end at a full stop or a question mark." stacked>
      {#snippet control()}
        <div class="grow">
          <Slider
            id="pause-length"
            label="Pause length"
            min={0.3}
            max={3}
            step={0.1}
            value={pauseDraft ?? config.asr.pause_s}
            format={(value) => `${value.toFixed(1)} s`}
            oninput={(value) => (pauseDraft = value)}
            onchange={async (value) => {
              await session.save("asr.pause_s", asrPatch({ pause_s: Math.round(value * 10) / 10 }));
              pauseDraft = null;
            }}
          />
        </div>
      {/snippet}
    </Row>
    <Row title="Even out quiet and loud voices">
      {#snippet control()}
        <Switch label="Even out quiet and loud voices" checked={config.audio.normalize} onchange={(normalize) => session.save("audio.normalize", { audio: { normalize } })} />
      {/snippet}
    </Row>
    <details class="advanced">
      <summary>Advanced</summary>
      <Row title="Speech threads" help="More can help on a busy PC. Most PCs are fine with 1.">
        {#snippet control()}
          <select class="field" aria-label="Speech threads" data-testid="asr-threads" value={String(config.asr.threads)} onchange={(event) => chooseThreads(event, "threads")}>
            {#each threadOptions(config.asr.threads) as count (count)}<option value={String(count)}>{count}</option>{/each}
          </select>
        {/snippet}
      </Row>
      {#if config.asr.second_pass}
        <Row title="Double-check threads">
          {#snippet control()}
            <select class="field" aria-label="Double-check threads" data-testid="asr-second-pass-threads" value={String(config.asr.second_pass_threads)} onchange={(event) => chooseThreads(event, "second_pass_threads")}>
              {#each threadOptions(config.asr.second_pass_threads) as count (count)}<option value={String(count)}>{count}</option>{/each}
            </select>
          {/snippet}
        </Row>
      {/if}
    </details>
  </Section>
</section>

<style>
  .apps {
    display: flex;
    flex-direction: column;
    padding: 0 18px 8px;
  }

  .app {
    display: grid;
    grid-template-columns: auto auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    border-top: 1.5px dashed var(--line);
    cursor: pointer;
  }

  .app input {
    accent-color: var(--pen-text);
    width: 18px;
    height: 18px;
  }

  .icon {
    width: 24px;
    height: 24px;
    border-radius: 7px;
    background: var(--paper-2);
  }

  .level {
    color: var(--ink-3);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }

  .empty {
    padding: 8px 0;
  }

  .grow {
    width: 100%;
  }
</style>
