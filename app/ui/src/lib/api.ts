import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { parseConfig, type Config, type ConfigPatch, type SetConfigResult } from "./settings";
import type {
  AppState,
  AudioAppList,
  AudioDevice,
  FolderKind,
  ModelSource,
  ModelStatus,
  PipelineStats,
} from "./types";

export const COMMANDS = {
  getState: "get_state",
  startListening: "start_listening",
  pauseListening: "pause_listening",
  getConfig: "get_config",
  setConfig: "set_config",
  getStats: "get_stats",
  startupNotices: "startup_notices",
  listAudioDevices: "list_audio_devices",
  listAudioApps: "list_audio_apps",
  modelsStatus: "models_status",
  modelsDownload: "models_download",
  modelsPause: "models_pause",
  modelsUseExisting: "models_use_existing",
  setOverlayMoving: "set_overlay_moving",
  setOverlayVisible: "set_overlay_visible",
  openFolder: "open_folder",
  showControlWindow: "show_control_window",
  quit: "quit",
} as const;

export type CommandName = (typeof COMMANDS)[keyof typeof COMMANDS];

export class ApiError extends Error {
  readonly command: CommandName;

  constructor(command: CommandName, message: string) {
    super(message);
    this.name = "ApiError";
    this.command = command;
  }
}

const GENERIC_FAILURE = "Something went wrong. Please try again.";

export function readableError(error: unknown): string {
  if (typeof error === "string") return error.trim() || GENERIC_FAILURE;
  if (error instanceof TypeError && /__TAURI|invoke|undefined/.test(error.message)) {
    return "The app backend is not running.";
  }
  if (error instanceof Error) return error.message.trim() || GENERIC_FAILURE;
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = (error as { message: unknown }).message;
    if (typeof message === "string" && message.trim() !== "") return message.trim();
  }
  return GENERIC_FAILURE;
}

async function call<T>(command: CommandName, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw new ApiError(command, readableError(error));
  }
}

export const getState = (): Promise<AppState> => call<AppState>(COMMANDS.getState);
/** Config problems found at launch plus hardware warnings, as plain sentences. */
export const startupNotices = (): Promise<string[]> => call<string[]>(COMMANDS.startupNotices);
export const startListening = (): Promise<void> => call<void>(COMMANDS.startListening);
export const pauseListening = (): Promise<void> => call<void>(COMMANDS.pauseListening);
export const getStats = (): Promise<PipelineStats> => call<PipelineStats>(COMMANDS.getStats);
export const listAudioDevices = (): Promise<AudioDevice[]> => call<AudioDevice[]>(COMMANDS.listAudioDevices);
export const listAudioApps = (): Promise<AudioAppList> => call<AudioAppList>(COMMANDS.listAudioApps);
export const modelsStatus = (): Promise<ModelStatus[]> => call<ModelStatus[]>(COMMANDS.modelsStatus);
export const modelsDownload = (source: ModelSource): Promise<void> => call<void>(COMMANDS.modelsDownload, { source });
export const modelsPause = (): Promise<void> => call<void>(COMMANDS.modelsPause);
export const modelsUseExisting = (folder: string): Promise<ModelStatus[]> =>
  call<ModelStatus[]>(COMMANDS.modelsUseExisting, { folder });
export const setOverlayMoving = (moving: boolean): Promise<void> => call<void>(COMMANDS.setOverlayMoving, { moving });
export const setOverlayVisible = (visible: boolean): Promise<void> =>
  call<void>(COMMANDS.setOverlayVisible, { visible });
export const openFolder = (which: FolderKind): Promise<void> => call<void>(COMMANDS.openFolder, { which });
/** Not in 07: shows and focuses the fixed "control" window (panel header Settings button). */
export const showControlWindow = (): Promise<void> => call<void>(COMMANDS.showControlWindow);
export const quit = (): Promise<void> => call<void>(COMMANDS.quit);

export async function getConfig(): Promise<Config> {
  const raw = await call<unknown>(COMMANDS.getConfig);
  try {
    return parseConfig(raw);
  } catch (error) {
    throw new ApiError(COMMANDS.getConfig, readableError(error));
  }
}

export async function setConfig(patch: ConfigPatch): Promise<SetConfigResult> {
  const result = await call<SetConfigResult>(COMMANDS.setConfig, { patch });
  try {
    return { ...result, config: parseConfig(result.config), messages: result.messages ?? [] };
  } catch (error) {
    throw new ApiError(COMMANDS.setConfig, readableError(error));
  }
}

/** Folder chooser for "Use files I already have". Resolves null when the user cancels. */
export async function pickFolder(title: string): Promise<string | null> {
  try {
    const picked = await open({ directory: true, multiple: false, title });
    return typeof picked === "string" ? picked : null;
  } catch (error) {
    throw new ApiError(COMMANDS.modelsUseExisting, readableError(error));
  }
}
