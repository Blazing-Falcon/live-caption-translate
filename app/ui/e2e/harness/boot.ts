import { emit } from "@tauri-apps/api/event";
import { applyPatch, type ConfigPatch } from "../../src/lib/settings";
import type { AppState, AudioAppList, ModelStatus, PipelineStats } from "../../src/lib/types";
import { FakeBackend } from "../../tests/backend";

export interface Boot {
  config?: ConfigPatch;
  state?: Partial<AppState>;
  models?: ModelStatus[];
  apps?: AudioAppList;
  stats?: PipelineStats;
}

declare global {
  interface Window {
    __LT_BOOT?: Boot;
    __lt: {
      backend: FakeBackend;
      emit: (name: string, payload: unknown) => Promise<void>;
    };
  }
}

const boot = window.__LT_BOOT ?? {};
const backend = new FakeBackend();
if (boot.config) backend.config = applyPatch(backend.config, boot.config);
if (boot.state) backend.state = { ...backend.state, ...boot.state };
if (boot.models) backend.models = boot.models;
if (boot.apps) backend.apps = boot.apps;
if (boot.stats) backend.stats = boot.stats;
backend.install();

window.__lt = { backend, emit: (name, payload) => emit(name, payload) };
