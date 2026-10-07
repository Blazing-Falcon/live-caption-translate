<script lang="ts">
  import { onMount } from "svelte";
  import { get } from "svelte/store";
  import * as api from "../../lib/api";
  import { useSession } from "../../lib/context";
  import { levelText } from "../../lib/format";
  import { reconcileChecked, reconcileRadios } from "../../lib/reconcile";
  import {
    captureAppsPatch,
    captureDevicePatch,
    captureModePatch,
    hasApp,
    toggledApps,
    type CaptureApp,
    type Config,
  } from "../../lib/settings";
  import type { AudioApp, AudioAppList, AudioDevice } from "../../lib/types";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { config: liveConfig } = session;

  let devices = $state<AudioDevice[]>([]);
  let appList = $state<AudioAppList | null>(null);
  let appError = $state<string | null>(null);

  const mode = $derived(config.capture.mode);
  const unsupported = $derived(appList !== null && !appList.supported);
  const reason = $derived(appList?.reason ?? "Selected apps needs Windows 11.");
  const defaultDevice = $derived(devices.find((device) => device.is_default));
  const deviceKnown = $derived(
    config.capture.device === "default" || devices.some((device) => device.id === config.capture.device),
  );

  interface Row {
    exe: string;
    name: string;
    icon: string | null;
    level: string;
  }

  const rows = $derived.by<Row[]>(() => {
    const live: Row[] = (appList?.apps ?? []).map((app: AudioApp) => ({
      exe: app.exe,
      name: app.name,
      icon: app.icon_png,
      level: levelText(app),
    }));
    const remembered: Row[] = config.capture.apps
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
    api.listAudioDevices().then((list) => (devices = list)).catch((error: unknown) => {
      session.pushNotice("error", api.readableError(error));
    });
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

  async function chooseMode(event: Event, next: "system" | "apps"): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    await session.save("capture.mode", captureModePatch(next));
    reconcileRadios(input, get(liveConfig)?.capture.mode ?? mode);
  }

  async function chooseDevice(event: Event): Promise<void> {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await session.save("capture.device", captureDevicePatch(select.value));
    if (!saved) select.value = config.capture.device;
  }

  async function toggleApp(event: Event, row: Row): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const entry: CaptureApp = { exe: row.exe, name: row.name };
    await session.save("capture.apps", captureAppsPatch(toggledApps(config.capture.apps, entry, input.checked)));
    reconcileChecked(input, hasApp(get(liveConfig)?.capture.apps ?? [], row.exe));
  }
</script>

<fieldset class="page" aria-label="Capture">
  <legend class="section-title">Listen to</legend>
  <div class="stack">
    <label class="option-card">
      <input
        type="radio"
        name="source"
        value="system"
        checked={mode === "system"}
        onchange={(event) => chooseMode(event, "system")}
      />
      <span class="option-text">
        <span class="option-title">Whole system</span>
        <span class="option-desc">Everything playing on one output device</span>
      </span>
    </label>
    <label class="option-card">
      <input
        type="radio"
        name="source"
        value="apps"
        checked={mode === "apps"}
        disabled={unsupported}
        onchange={(event) => chooseMode(event, "apps")}
      />
      <span class="option-text">
        <span class="option-title">Selected apps</span>
        <span class="option-desc">Only the apps you tick, mixed together</span>
      </span>
    </label>
    {#if unsupported}
      <p class="help warning" data-testid="apps-unsupported">
        {reason}
        {#if mode === "apps"}
          Listening to the whole system instead; your picks are kept.
        {/if}
      </p>
    {/if}
  </div>

  {#if mode === "system" || unsupported}
    <label class="field">
      <span>Output device</span>
      <select class="select" value={config.capture.device} onchange={chooseDevice}>
        <option value="default">
          {defaultDevice ? `Follow Windows default (${defaultDevice.name})` : "Follow Windows default"}
        </option>
        {#each devices as device (device.id)}
          <option value={device.id}>{device.name}</option>
        {/each}
        {#if !deviceKnown}
          <option value={config.capture.device}>Saved device (not connected)</option>
        {/if}
      </select>
    </label>
  {/if}

  {#if mode === "apps" && !unsupported}
    <div class="list" role="group" aria-label="Apps playing sound now">
      <div class="list-head">Apps playing sound now</div>
      {#each rows as row (row.exe.toLowerCase())}
        <label class="list-row">
          <input
            type="checkbox"
            checked={hasApp(config.capture.apps, row.exe)}
            onchange={(event) => toggleApp(event, row)}
          />
          {#if row.icon}
            <img class="app-icon" alt="" src={`data:image/png;base64,${row.icon}`} />
          {/if}
          <span class="name">{row.name}</span>
          <span class="level">{row.level}</span>
        </label>
      {:else}
        <div class="list-row">No apps are playing sound right now.</div>
      {/each}
      {#if appError}
        <div class="list-foot warning">{appError}</div>
      {/if}
      <div class="list-foot">Picked apps are remembered and reconnect when they restart. Needs Windows 11.</div>
    </div>
  {/if}
</fieldset>
