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

const MB = 1024 ** 2;

export const READY_MODELS: ModelStatus[] = [
  { id: "xasr-stream-480ms-int8-2026-06-05", name: "Speech recognition (480 ms)", bytes_total: 128 * MB, bytes_done: 128 * MB, optional: false, state: "ready" },
  { id: "hy-mt2-1.8b-ternary-tq2_0", name: "Translation (ternary)", bytes_total: 440 * MB, bytes_done: 440 * MB, optional: false, state: "ready" },
  { id: "hy-mt2-1.8b-q4_0", name: "Translation (Q4_0)", bytes_total: 1030 * MB, bytes_done: 0, optional: true, state: "missing" },
  { id: "qwen3-asr-0.6b-q4_k_m", name: "Second-pass speech recognition (Qwen3-ASR)", bytes_total: 666 * MB, bytes_done: 666 * MB, optional: false, state: "ready" },
  { id: "xasr-stream-960ms-int8-2026-06-05", name: "Speech recognition (960 ms)", bytes_total: 128 * MB, bytes_done: 0, optional: true, state: "missing" },
  { id: "xasr-stream-160ms-int8-2026-06-05", name: "Speech recognition (160 ms)", bytes_total: 128 * MB, bytes_done: 0, optional: true, state: "missing" },
];

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
