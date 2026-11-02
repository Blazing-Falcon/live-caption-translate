import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { parseConfig, type Config, type ConfigPatch, type SetConfigResult } from "./settings";
import type {
  SavedLine,
  SavedSession,
  AppState,
  AudioAppList,
  AudioDevice,
  FolderKind,
  ModelStatus,
  PipelineStats,
  Rect,
  ScreenPlate,
  ScreenPlateRequest,
  ScreenDirection,
} from "./types";

export const COMMANDS = {
  getState: "get_state",
  startListening: "start_listening",
  pauseListening: "pause_listening",
  getConfig: "get_config",
  testTranslator: "test_translator",
  setConfig: "set_config",
  resetConfig: "reset_config",
  getStats: "get_stats",
  startupNotices: "startup_notices",
  listAudioDevices: "list_audio_devices",
  listAudioApps: "list_audio_apps",
  modelsStatus: "models_status",
  modelsDownload: "models_download",
  modelsDownloadOptional: "models_download_optional",
  modelsPause: "models_pause",
  modelsUseExisting: "models_use_existing",
  screenSelect: "screen_select",
  screenPlate: "screen_plate",
  screenClose: "screen_close",
  setOverlayMoving: "set_overlay_moving",
  setOverlayVisible: "set_overlay_visible",
  openFolder: "open_folder",
  listTranscripts: "list_transcripts",
  readTranscript: "read_transcript",
  openSettings: "open_settings",
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

export const testTranslator = (): Promise<{ text: string; ms: number }> => call(COMMANDS.testTranslator);
export const getState = (): Promise<AppState> => call<AppState>(COMMANDS.getState);
/** Config problems found at launch plus hardware warnings, as plain sentences. */
export const startupNotices = (): Promise<string[]> => call<string[]>(COMMANDS.startupNotices);
export const startListening = (): Promise<void> => call<void>(COMMANDS.startListening);
export const pauseListening = (): Promise<void> => call<void>(COMMANDS.pauseListening);
export const getStats = (): Promise<PipelineStats> => call<PipelineStats>(COMMANDS.getStats);
export const listAudioDevices = (): Promise<AudioDevice[]> => call<AudioDevice[]>(COMMANDS.listAudioDevices);
export const listAudioApps = (): Promise<AudioAppList> => call<AudioAppList>(COMMANDS.listAudioApps);
export const modelsStatus = (): Promise<ModelStatus[]> => call<ModelStatus[]>(COMMANDS.modelsStatus);
/** Every missing required model (first run). */
export const modelsDownload = (): Promise<void> => call<void>(COMMANDS.modelsDownload);
/** One model by id, also an optional one such as the screen text recognizer. */
export const modelsDownloadOptional = (id: string): Promise<void> => call<void>(COMMANDS.modelsDownloadOptional, { id });
/** Starts a job on `rect` (frame pixels) of frame `frame`; resolves with the job id. */
export const screenSelect = (frame: number, rect: Rect, debug = false, direction?: ScreenDirection): Promise<number> =>
  call<number>(COMMANDS.screenSelect, { frame, rect, ...(debug ? { debug: true } : {}), ...(direction ? { direction } : {}) });
export const screenClose = (): Promise<void> => call<void>(COMMANDS.screenClose);
export const screenPlate = (request: ScreenPlateRequest): Promise<ScreenPlate> =>
  call<ScreenPlate>(COMMANDS.screenPlate, { request });
export const modelsPause = (): Promise<void> => call<void>(COMMANDS.modelsPause);
export const modelsUseExisting = (folder: string): Promise<ModelStatus[]> =>
  call<ModelStatus[]>(COMMANDS.modelsUseExisting, { folder });
export const setOverlayMoving = (moving: boolean): Promise<void> => call<void>(COMMANDS.setOverlayMoving, { moving });
export const setOverlayVisible = (visible: boolean): Promise<void> =>
  call<void>(COMMANDS.setOverlayVisible, { visible });
export const openFolder = (which: FolderKind): Promise<void> => call<void>(COMMANDS.openFolder, { which });
export const listTranscripts = (): Promise<SavedSession[]> => call<SavedSession[]>(COMMANDS.listTranscripts);
export const readTranscript = (file: string): Promise<SavedLine[]> => call<SavedLine[]>(COMMANDS.readTranscript, { file });
export const openSettings = (): Promise<void> => call<void>(COMMANDS.openSettings);
export const quit = (): Promise<void> => call<void>(COMMANDS.quit);

export async function getConfig(): Promise<Config> {
  const raw = await call<unknown>(COMMANDS.getConfig);
  try {
    return parseConfig(raw);
  } catch (error) {
    throw new ApiError(COMMANDS.getConfig, readableError(error));
  }
}

export const setConfig = (patch: ConfigPatch): Promise<SetConfigResult> => changeConfig(COMMANDS.setConfig, { patch });

/** Back to the defaults; the overlay positions, the chosen apps and the models folder stay. */
export const resetConfig = (): Promise<SetConfigResult> => changeConfig(COMMANDS.resetConfig);

async function changeConfig(command: CommandName, args?: Record<string, unknown>): Promise<SetConfigResult> {
  const result = await call<SetConfigResult>(command, args);
  try {
    return { ...result, config: parseConfig(result.config), messages: result.messages ?? [] };
  } catch (error) {
    throw new ApiError(command, readableError(error));
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
