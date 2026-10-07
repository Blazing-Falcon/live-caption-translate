import { mockIPC } from "@tauri-apps/api/mocks";
import { applyPatch, type AppliedKind, type Config, type ConfigPatch, type SetConfigResult } from "../src/lib/settings";
import type { AppState, AudioAppList, AudioDevice, ModelStatus, PipelineStats } from "../src/lib/types";
import { STATS, testConfig, testState } from "./helpers";

export interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

const PNG_PIXEL =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

export const READY_MODELS: ModelStatus[] = [
  { id: "vad", name: "Voice detection", bytes_total: 2 * 1024 ** 2, bytes_done: 2 * 1024 ** 2, state: "ready" },
  { id: "asr", name: "Speech recognition", bytes_total: 239 * 1024 ** 2, bytes_done: 239 * 1024 ** 2, state: "ready" },
  { id: "mt", name: "Translation", bytes_total: 1.08 * 1024 ** 3, bytes_done: 1.08 * 1024 ** 3, state: "ready" },
];

export const MISSING_MODELS: ModelStatus[] = READY_MODELS.map((model, index) => ({
  ...model,
  bytes_done: index === 0 ? model.bytes_total : 0,
  state: index === 0 ? "ready" : "missing",
}));

/** In-process stand-in for the Rust side; every command the UI can call is answered here. */
export class FakeBackend {
  calls: Call[] = [];
  state: AppState = testState();
  config: Config = testConfig();
  models: ModelStatus[] = structuredClone(READY_MODELS);
  stats: PipelineStats = STATS;
  applied: AppliedKind = "live";
  messages: string[] = [];
  devices: AudioDevice[] = [
    { id: "dev-speakers", name: "Speakers (Realtek)", is_default: true },
    { id: "dev-usb", name: "Headphones (USB Audio)", is_default: false },
  ];
  apps: AudioAppList = {
    supported: true,
    reason: null,
    apps: [
      { exe: "chrome.exe", name: "Google Chrome", pid: 1, icon_png: PNG_PIXEL, peak: 0.5, active: true, recent: false },
      { exe: "vlc.exe", name: "VLC media player", pid: 2, icon_png: null, peak: 0.0, active: true, recent: true },
    ],
  };
  failures = new Map<string, string>();
  dialogResult: string | null = "D:\\existing-models";

  install(): this {
    mockIPC((cmd, args) => this.handle(cmd, (args ?? {}) as Record<string, unknown>), { shouldMockEvents: true });
    return this;
  }

  count(cmd: string): number {
    return this.calls.filter((call) => call.cmd === cmd).length;
  }

  last(cmd: string): Call | undefined {
    return this.calls.filter((call) => call.cmd === cmd).at(-1);
  }

  private handle(cmd: string, args: Record<string, unknown>): unknown {
    this.calls.push({ cmd, args });
    const failure = this.failures.get(cmd);
    if (failure !== undefined) throw failure;
    switch (cmd) {
      case "get_state":
        return this.state;
      case "get_config":
        return this.config;
      case "get_stats":
        return this.stats;
      case "set_config": {
        this.config = applyPatch(this.config, args.patch as ConfigPatch);
        const result: SetConfigResult = { config: this.config, applied: this.applied, messages: this.messages };
        return result;
      }
      case "models_status":
        return this.models;
      case "models_use_existing":
        this.models = this.models.map((model) => ({ ...model, bytes_done: model.bytes_total, state: "ready" }));
        return this.models;
      case "list_audio_apps":
        return this.apps;
      case "list_audio_devices":
        return this.devices;
      case "plugin:dialog|open":
        return this.dialogResult;
      default:
        return null;
    }
  }
}
