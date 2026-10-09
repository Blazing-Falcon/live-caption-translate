// @vitest-environment jsdom
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import * as api from "../src/lib/api";
import { DEFAULT_CONFIG } from "../src/lib/settings";
import { testState } from "./helpers";

type Call = { cmd: string; args: Record<string, unknown> | undefined };

function mock(handler: (cmd: string, args: Record<string, unknown> | undefined) => unknown = () => null): Call[] {
  const calls: Call[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: args as Record<string, unknown> | undefined });
    return handler(cmd, args as Record<string, unknown> | undefined);
  });
  return calls;
}

afterEach(() => clearMocks());

describe("command wrappers", () => {
  it("send the documented command names and argument shapes", async () => {
    const calls = mock((cmd) => {
      if (cmd === "get_state") return testState();
      if (cmd === "get_config") return DEFAULT_CONFIG;
      if (cmd === "set_config") return { config: DEFAULT_CONFIG, applied: "live", messages: [] };
      if (cmd === "list_audio_apps") return { supported: true, reason: null, apps: [] };
      if (cmd === "list_audio_devices" || cmd === "models_status" || cmd === "models_use_existing") return [];
      return null;
    });

    await api.getState();
    await api.getConfig();
    await api.setConfig({ overlay: { font_px: 30 } });
    await api.startListening();
    await api.pauseListening();
    await api.modelsStatus();
    await api.modelsDownload("modelscope");
    await api.modelsDownload("huggingface", [api.DRAFT_MODEL_ID]);
    await api.modelsPause();
    await api.modelsUseExisting("D:\\models");
    await api.listAudioApps();
    await api.listAudioDevices();
    await api.setOverlayVisible(false);
    await api.setOverlayMoving(true);
    await api.openFolder("transcripts");
    await api.showControlWindow();
    await api.quit();
    await api.startupNotices();

    expect(calls).toEqual([
      { cmd: "get_state", args: {} },
      { cmd: "get_config", args: {} },
      { cmd: "set_config", args: { patch: { overlay: { font_px: 30 } } } },
      { cmd: "start_listening", args: {} },
      { cmd: "pause_listening", args: {} },
      { cmd: "models_status", args: {} },
      { cmd: "models_download", args: { source: "modelscope" } },
      { cmd: "models_download", args: { source: "huggingface", ids: ["lmt-60-0.6b-q4_k_m"] } },
      { cmd: "models_pause", args: {} },
      { cmd: "models_use_existing", args: { folder: "D:\\models" } },
      { cmd: "list_audio_apps", args: {} },
      { cmd: "list_audio_devices", args: {} },
      { cmd: "set_overlay_visible", args: { visible: false } },
      { cmd: "set_overlay_moving", args: { moving: true } },
      { cmd: "open_folder", args: { which: "transcripts" } },
      { cmd: "show_control_window", args: {} },
      { cmd: "quit", args: {} },
      { cmd: "startup_notices", args: {} },
    ]);
  });

  it("covers every command name in COMMANDS", () => {
    expect(Object.values(api.COMMANDS).sort()).toEqual(
      [
        "get_state", "start_listening", "pause_listening", "get_config", "set_config", "get_stats",
        "list_audio_devices", "list_audio_apps", "models_status", "models_download", "models_pause",
        "models_use_existing", "set_overlay_moving", "set_overlay_visible", "open_folder", "show_control_window", "quit",
        "startup_notices",
      ].sort(),
    );
  });

  it("returns typed results unchanged", async () => {
    mock((cmd) => (cmd === "list_audio_apps" ? { supported: false, reason: "Needs Windows 11", apps: [] } : testState()));
    expect(await api.listAudioApps()).toEqual({ supported: false, reason: "Needs Windows 11", apps: [] });
    expect((await api.getState()).listening).toBe("listening");
  });

  it("returns the normalized config and messages from set_config", async () => {
    mock(() => ({ config: { ...DEFAULT_CONFIG, overlay: { ...DEFAULT_CONFIG.overlay, font_px: 40 } }, applied: "restart" }));
    const result = await api.setConfig({ overlay: { font_px: 99 } });
    expect(result.config.overlay.font_px).toBe(40);
    expect(result.applied).toBe("restart");
    expect(result.messages).toEqual([]);
  });
});

describe("errors", () => {
  it("turns string rejections into readable ApiErrors", async () => {
    mock(() => {
      throw "Models are missing. Download them first.";
    });
    const error = await api.startListening().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(api.ApiError);
    expect((error as api.ApiError).message).toBe("Models are missing. Download them first.");
    expect((error as api.ApiError).command).toBe("start_listening");
  });

  it("maps assorted failure shapes to a sentence", () => {
    expect(api.readableError("  Could not open folder.  ")).toBe("Could not open folder.");
    expect(api.readableError("")).toBe("Something went wrong. Please try again.");
    expect(api.readableError(new Error("Disk full"))).toBe("Disk full");
    expect(api.readableError({ message: "From an object" })).toBe("From an object");
    expect(api.readableError(42)).toBe("Something went wrong. Please try again.");
    expect(api.readableError(new TypeError("Cannot read properties of undefined (reading 'invoke')"))).toBe(
      "The app backend is not running.",
    );
  });

  it("rejects an unreadable config payload", async () => {
    mock(() => ({ nonsense: true }));
    await expect(api.getConfig()).rejects.toThrow(/unreadable/);
    await expect(api.setConfig({})).rejects.toBeInstanceOf(api.ApiError);
  });

  it("reports a missing backend in plain words", async () => {
    clearMocks();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    const error = await api.getState().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(api.ApiError);
    expect((error as api.ApiError).message).toBe("The app backend is not running.");
  });
});

describe("pickFolder", () => {
  it("opens a directory-only dialog and returns the chosen path", async () => {
    const calls = mock((cmd) => (cmd === "plugin:dialog|open" ? "D:\\models" : null));
    expect(await api.pickFolder("Choose")).toBe("D:\\models");
    expect(calls[0]?.cmd).toBe("plugin:dialog|open");
    expect(calls[0]?.args).toMatchObject({ options: { directory: true, multiple: false, title: "Choose" } });
  });

  it("returns null when the user cancels", async () => {
    mock(() => null);
    expect(await api.pickFolder("Choose")).toBeNull();
  });
});
