// @vitest-environment jsdom
import { emit } from "@tauri-apps/api/event";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { createSession, EVENTS, type Session } from "../src/lib/events";
import ControlApp from "../src/control/ControlApp.svelte";
import { FakeBackend, MISSING_MODELS, READY_MODELS } from "./backend";
import { byRole, cleanup, q, render, setValue, settle, text } from "./dom";
import { FakeClock, testState } from "./helpers";

const sessions: Session[] = [];

async function open(configure?: (backend: FakeBackend) => void) {
  const backend = new FakeBackend();
  configure?.(backend);
  backend.install();
  const clock = new FakeClock();
  const session = createSession({ kind: "control", clock, reducedMotion: () => false });
  sessions.push(session);
  const root = render(ControlApp, { session });
  await session.ready;
  await settle();
  return { backend, clock, session, root };
}

const tab = (root: HTMLElement, name: string): HTMLElement => byRole(root, "tab", name);

async function go(root: HTMLElement, name: string): Promise<void> {
  tab(root, name).click();
  await settle();
}

afterEach(() => {
  sessions.splice(0).forEach((session) => session.dispose());
  cleanup();
  clearMocks();
});

describe("shell", () => {
  it("shows the four pages, a header card and the monospace footer", async () => {
    const { root } = await open();
    expect([...root.querySelectorAll('[role="tab"]')].map((el) => el.textContent?.trim())).toEqual([
      "Capture",
      "Overlay",
      "Languages",
      "Performance",
    ]);
    expect(text(q(root, '[data-testid="header-title"]'))).toBe("Listening");
    expect(text(q(root, '[data-testid="header-sub"]'))).toBe("Chinese to English · Speakers (Realtek)");
    expect(text(q(root, '[data-testid="footer"]'))).toBe("speech→English 1.8 s median · queue 1 · CPU 41%");
  });

  it("keeps header and footer on every page", async () => {
    const { root } = await open();
    for (const name of ["Overlay", "Languages", "Performance", "Capture"]) {
      await go(root, name);
      expect(root.querySelector('[data-testid="header-card"]')).not.toBeNull();
      expect(root.querySelector('[data-testid="footer"]')).not.toBeNull();
      expect(tab(root, name).getAttribute("aria-selected")).toBe("true");
    }
  });

  it("supports arrow, Home and End keys on the page list with a roving tabindex", async () => {
    const { root } = await open();
    const key = async (name: string, from: string) => {
      tab(root, from).dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
      await settle();
    };
    await key("ArrowDown", "Capture");
    expect(tab(root, "Overlay").getAttribute("aria-selected")).toBe("true");
    expect(tab(root, "Overlay").getAttribute("tabindex")).toBe("0");
    expect(tab(root, "Capture").getAttribute("tabindex")).toBe("-1");
    expect(document.activeElement).toBe(tab(root, "Overlay"));
    await key("End", "Overlay");
    expect(tab(root, "Performance").getAttribute("aria-selected")).toBe("true");
    await key("ArrowDown", "Performance");
    expect(tab(root, "Capture").getAttribute("aria-selected")).toBe("true");
    await key("ArrowUp", "Capture");
    expect(tab(root, "Performance").getAttribute("aria-selected")).toBe("true");
    await key("Home", "Performance");
    expect(tab(root, "Capture").getAttribute("aria-selected")).toBe("true");
    expect(q(root, '[role="tabpanel"]').getAttribute("aria-labelledby")).toBe("tab-capture");
  });

  it("pauses and starts through the header button and shows a readable error on failure", async () => {
    const { root, backend, session } = await open();
    byRole(root, "button", "Pause").click();
    await settle();
    expect(backend.count("pause_listening")).toBe(1);
    await emit(EVENTS.pipeline, { type: "listening_state", state: "paused" });
    await settle();
    expect(text(q(root, '[data-testid="header-title"]'))).toBe("Paused");
    expect(text(q(root, '[data-testid="header-sub"]'))).toBe("Captions are off. Models stay loaded.");
    expect(text(q(root, '[data-testid="footer"]'))).toBe("paused");
    backend.failures.set("start_listening", "Models are missing. Download them first.");
    byRole(root, "button", "Start").click();
    await settle();
    expect(text(q(root, '[role="alert"]'))).toContain("Models are missing. Download them first.");
    expect(session.dismissNotice).toBeTypeOf("function");
    byRole(root, "button", "Dismiss message").click();
    await settle();
    expect(root.querySelector('[role="alert"]')).toBeNull();
  });

  it("shows a retry when nothing can be loaded", async () => {
    const { root } = await open((b) => b.failures.set("get_config", "The app is still starting."));
    expect(text(q(root, '[role="alert"]'))).toContain("The app is still starting.");
    expect(byRole(root, "button", "Try again")).toBeTruthy();
  });
});

describe("Capture", () => {
  it("reflects the saved mode with native radios and saves a mode change", async () => {
    const { root, backend } = await open();
    expect((byRole(root, "radio", /Whole system/) as HTMLInputElement).checked).toBe(true);
    byRole(root, "radio", /Selected apps/).click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { capture: { mode: "apps" } } });
    expect((byRole(root, "radio", /Selected apps/) as HTMLInputElement).checked).toBe(true);
    expect((byRole(root, "radio", /Whole system/) as HTMLInputElement).checked).toBe(false);
  });

  it("keeps the old radio when saving fails", async () => {
    const { root, backend } = await open();
    backend.failures.set("set_config", "Could not save settings.");
    byRole(root, "radio", /Selected apps/).click();
    await settle();
    expect((byRole(root, "radio", /Whole system/) as HTMLInputElement).checked).toBe(true);
    expect((byRole(root, "radio", /Selected apps/) as HTMLInputElement).checked).toBe(false);
    expect(text(q(root, '[role="alert"]'))).toContain("Could not save settings.");
  });

  it("lists devices with the Windows default option and saves a choice", async () => {
    const { root, backend } = await open();
    const select = byRole(root, "combobox", "Output device") as HTMLSelectElement;
    expect([...select.options].map((o) => o.textContent?.trim())).toEqual([
      "Follow Windows default (Speakers (Realtek))",
      "Speakers (Realtek)",
      "Headphones (USB Audio)",
    ]);
    setValue(select, "dev-usb", ["change"]);
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { capture: { device: "dev-usb" } } });
  });

  it("keeps a saved device that is no longer connected selectable", async () => {
    const { root } = await open((b) => (b.config.capture.device = "gone-device"));
    const select = byRole(root, "combobox", "Output device") as HTMLSelectElement;
    expect(select.value).toBe("gone-device");
    expect(select.options[select.selectedIndex]?.textContent?.trim()).toBe("Saved device (not connected)");
  });

  it("disables Selected apps with the supplied reason while keeping the saved mode", async () => {
    const { root } = await open((b) => {
      b.config.capture.mode = "apps";
      b.apps = { supported: false, reason: "Selected apps needs Windows 11 (build 20348 or later).", apps: [] };
    });
    const apps = byRole(root, "radio", /Selected apps/) as HTMLInputElement;
    expect(apps.disabled).toBe(true);
    expect(apps.checked).toBe(true);
    expect(text(q(root, '[data-testid="apps-unsupported"]'))).toContain("Selected apps needs Windows 11 (build 20348 or later).");
    expect(text(q(root, '[data-testid="apps-unsupported"]'))).toContain("Listening to the whole system");
    expect(byRole(root, "combobox", "Output device")).toBeTruthy();
  });

  it("lists apps with levels, icons and remembered picks, and saves the whole array", async () => {
    const { root, backend } = await open((b) => {
      b.config.capture.mode = "apps";
      b.config.capture.apps = [
        { exe: "CHROME.EXE", name: "Google Chrome" },
        { exe: "discord.exe", name: "Discord" },
      ];
    });
    const rows = [...root.querySelectorAll(".list-row")].map((row) => text(row));
    expect(rows).toEqual(["Google Chrome -6 dB", "VLC media player silent", "Discord not playing"]);
    expect(q<HTMLImageElement>(root, "img.app-icon").getAttribute("src")).toBe("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=");
    expect((byRole(root, "checkbox", /Google Chrome/) as HTMLInputElement).checked).toBe(true);
    expect((byRole(root, "checkbox", /VLC/) as HTMLInputElement).checked).toBe(false);
    byRole(root, "checkbox", /VLC/).click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({
      patch: {
        capture: {
          apps: [
            { exe: "CHROME.EXE", name: "Google Chrome" },
            { exe: "discord.exe", name: "Discord" },
            { exe: "vlc.exe", name: "VLC media player" },
          ],
        },
      },
    });
    byRole(root, "checkbox", /Google Chrome/).click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({
      patch: {
        capture: {
          apps: [
            { exe: "discord.exe", name: "Discord" },
            { exe: "vlc.exe", name: "VLC media player" },
          ],
        },
      },
    });
  });

  it("polls the app list about once a second while apps mode is open", async () => {
    const { backend, clock } = await open((b) => (b.config.capture.mode = "apps"));
    void clock;
    const initial = backend.count("list_audio_apps");
    expect(initial).toBeGreaterThanOrEqual(1);
    await new Promise((resolve) => setTimeout(resolve, 2300));
    await settle();
    expect(backend.count("list_audio_apps")).toBeGreaterThanOrEqual(initial + 2);
  });

  it("stops polling when leaving the page", async () => {
    const { root, backend } = await open((b) => (b.config.capture.mode = "apps"));
    await go(root, "Overlay");
    const after = backend.count("list_audio_apps");
    await new Promise((resolve) => setTimeout(resolve, 1500));
    expect(backend.count("list_audio_apps")).toBe(after);
  });
});

describe("Overlay page", () => {
  it("saves style, text size, background and the Chinese line toggle", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    expect((byRole(root, "radio", "Subtitle bar") as HTMLInputElement).checked).toBe(true);
    byRole(root, "radio", "Caption panel").click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { style: "panel" } } });

    const size = byRole(root, "slider", /English text size/) as HTMLInputElement;
    expect(size.min).toBe("18");
    expect(size.max).toBe("40");
    setValue(size, "34", ["input"]);
    await settle();
    expect(text(size.closest("label") ?? root)).toContain("34 px");
    setValue(size, "34", ["change"]);
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { font_px: 34 } } });

    const background = byRole(root, "slider", /Background/) as HTMLInputElement;
    setValue(background, "40");
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { background: 0.4 } } });

    byRole(root, "checkbox", /Show the Chinese line/).click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { show_source: false } } });
  });

  it("warns below 50% background without clamping", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    expect(root.querySelector('[data-testid="background-warning"]')).toBeNull();
    const background = byRole(root, "slider", /Background/) as HTMLInputElement;
    setValue(background, "10", ["input"]);
    await settle();
    expect(text(q(root, '[data-testid="background-warning"]'))).toBe("May be hard to read on bright video");
    setValue(background, "10", ["change"]);
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { background: 0.1 } } });
    expect(root.querySelector('[data-testid="background-warning"]')).not.toBeNull();
    setValue(background, "50");
    await settle();
    expect(root.querySelector('[data-testid="background-warning"]')).toBeNull();
  });

  it("reverts a slider when saving fails", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    backend.failures.set("set_config", "Could not save settings.");
    const size = byRole(root, "slider", /English text size/) as HTMLInputElement;
    setValue(size, "40");
    await settle();
    expect(text(size.closest("label") ?? root)).toContain("26 px");
    expect(size.value).toBe("26");
  });

  it("shows panel options only for the panel style", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    expect(root.querySelector("select")).toBeNull();
    byRole(root, "radio", "Caption panel").click();
    await settle();
    setValue(byRole(root, "combobox", "Lines kept in the panel") as HTMLSelectElement, "6", ["change"]);
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { panel_lines: 6 } } });
    setValue(byRole(root, "combobox", "Caption panel edge") as HTMLSelectElement, "left", ["change"]);
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { panel_edge: "left" } } });
  });

  it("drives the overlay window through visible/moving commands and follows emitted state", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    byRole(root, "checkbox", "Show the overlay").click();
    byRole(root, "button", "Move overlay").click();
    await settle();
    expect(backend.last("set_overlay_visible")?.args).toEqual({ visible: false });
    expect(backend.last("set_overlay_moving")?.args).toEqual({ moving: true });
    expect(backend.count("set_config")).toBe(0);
    await emit(EVENTS.overlayMode, { moving: true });
    await emit(EVENTS.overlayVisible, { visible: false });
    await settle();
    expect(byRole(root, "button", "Lock overlay").getAttribute("aria-pressed")).toBe("true");
    expect((byRole(root, "checkbox", "Show the overlay") as HTMLInputElement).checked).toBe(false);
  });

  it("rebinds hotkeys, allows empty to disable, and reports registration errors per action", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    const pause = byRole(root, "textbox", "Pause listening") as HTMLInputElement;
    const save = byRole(root, "button", "Save Pause listening shortcut") as HTMLButtonElement;
    expect(pause.value).toBe("Ctrl+Shift+P");
    expect(save.disabled).toBe(true);
    setValue(pause, "Ctrl+Alt+P", ["input"]);
    await settle();
    expect(save.disabled).toBe(false);
    save.click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { hotkeys: { pause: "Ctrl+Alt+P" } } });

    const showHide = byRole(root, "textbox", "Show or hide overlay") as HTMLInputElement;
    setValue(showHide, "", ["input"]);
    await settle();
    byRole(root, "button", "Save Show or hide overlay shortcut").click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { hotkeys: { show_hide: "" } } });

    await emit(EVENTS.hotkeysError, { action: "move_lock", accelerator: "Ctrl+Shift+L", message: "Another app is using it." });
    await settle();
    expect(text(q(root, '[role="alert"]'))).toBe(
      "Move or lock overlay: Ctrl+Shift+L could not be used. Another app is using it.",
    );
    expect((byRole(root, "textbox", "Move or lock overlay") as HTMLInputElement).getAttribute("aria-invalid")).toBe("true");
    expect(root.querySelector("main")).not.toBeNull();
  });
});

describe("Languages page", () => {
  it("shows the fixed languages and toggles Japanese/Korean while keeping unknown codes", async () => {
    const { root, backend } = await open((b) => (b.config.routing.translate_other = ["yue"]));
    await go(root, "Languages");
    expect(text(q(root, ".kv"))).toBe(
      "Speech language Chinese (Mandarin, Cantonese) Translate into English English speech Shown as heard",
    );
    expect((byRole(root, "checkbox", "Japanese") as HTMLInputElement).checked).toBe(false);
    byRole(root, "checkbox", "Japanese").click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { routing: { translate_other: ["yue", "ja"] } } });
    expect((byRole(root, "checkbox", "Japanese") as HTMLInputElement).checked).toBe(true);
    byRole(root, "checkbox", "Korean").click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { routing: { translate_other: ["yue", "ja", "ko"] } } });
    byRole(root, "checkbox", "Japanese").click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { routing: { translate_other: ["yue", "ko"] } } });
    expect(text(root)).toContain("confident nonsense");
  });
});

describe("Performance page", () => {
  it("shows real stats with the 07 labels", async () => {
    const { root } = await open();
    await go(root, "Performance");
    expect(text(q(root, ".stat-grid"))).toBe(
      "Speech end to English 1.8 s median, last 50 lines Waiting to translate 1 skipped this session: 3 Memory 1.9 GB CPU 41%, app and translator",
    );
  });

  it("shows placeholders before stats exist", async () => {
    const { root } = await open((b) => {
      b.failures.set("get_stats", "No stats yet.");
    });
    await go(root, "Performance");
    expect(text(q(root, '[data-testid="stat-median"]'))).toBe("—");
    expect(text(q(root, '[data-testid="footer"]'))).toBe("speech→English — median · queue — · CPU —");
  });

  it("saves thread count (0 = Automatic) and the transcript toggle, and opens the transcript folder", async () => {
    const { root, backend } = await open();
    await go(root, "Performance");
    const threads = byRole(root, "combobox", "Translator threads") as HTMLSelectElement;
    expect(threads.value).toBe("0");
    expect(threads.options[0]?.textContent?.trim()).toBe("Automatic");
    backend.applied = "pipeline";
    setValue(threads, "3", ["change"]);
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { translate: { threads: 3 } } });
    expect(text(q(root, '[role="status"]'))).toContain("restart briefly");
    backend.applied = "live";
    byRole(root, "checkbox", /Save a transcript/).click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { transcript: { enabled: false } } });
    byRole(root, "button", "Open transcript folder").click();
    await settle();
    expect(backend.last("open_folder")?.args).toEqual({ which: "transcripts" });
  });
});

describe("First run", () => {
  const firstRun = (b: FakeBackend) => {
    b.models = structuredClone(MISSING_MODELS);
    b.state = testState({ models_ready: false, listening: "paused" });
  };

  it("shows real model rows with sizes and states instead of the pages", async () => {
    const { root } = await open(firstRun);
    expect(root.querySelector('[role="tablist"]')).toBeNull();
    expect(text(q(root, "h1"))).toBe("Download the speech and translation models");
    const rows = [...root.querySelectorAll(".model")].map((row) => text(row));
    expect(rows).toEqual([
      "Voice detection 2.0 MB Done",
      "Speech recognition 239 MB Waiting",
      "Translation 1.08 GB Waiting",
    ]);
    expect(text(q(root, ".intro p"))).toContain("about 1.32 GB");
  });

  it("downloads from the chosen source and offers pause/resume as states change", async () => {
    const { root, backend } = await open(firstRun);
    const source = byRole(root, "combobox", "Download from") as HTMLSelectElement;
    setValue(source, "modelscope", ["change"]);
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { models: { source: "modelscope" } } });
    byRole(root, "button", "Download").click();
    await settle();
    expect(backend.last("models_download")?.args).toEqual({ source: "modelscope" });

    const mb = 1024 * 1024;
    const progress = (state: "downloading" | "paused" | "corrupt" | "verifying" | "ready", done = 152 * mb) => [
      { id: "vad", name: "Voice detection", bytes_total: 2 * mb, bytes_done: 2 * mb, state: "ready" as const },
      { id: "asr", name: "Speech recognition", bytes_total: 239 * mb, bytes_done: done, state },
      { id: "mt", name: "Translation", bytes_total: 1105 * mb, bytes_done: 0, state: "missing" as const },
    ];
    await emit(EVENTS.modelsProgress, progress("downloading"));
    await settle();
    expect(text(root.querySelectorAll(".model")[1] as HTMLElement)).toBe("Speech recognition 152 MB / 239 MB 64%");
    expect(q(root, '[role="progressbar"]').getAttribute("aria-valuenow")).toBe("64");

    byRole(root, "button", "Pause").click();
    await settle();
    expect(backend.count("models_pause")).toBe(1);
    await emit(EVENTS.modelsProgress, progress("paused"));
    await settle();
    expect(text(root.querySelectorAll(".model")[1] as HTMLElement)).toContain("Paused · 64%");
    byRole(root, "button", "Resume").click();
    await settle();
    expect(backend.count("models_download")).toBe(2);
  });

  it("marks verifying and corrupt rows honestly and retries corrupt ones", async () => {
    const { root, backend } = await open(firstRun);
    const mb = 1024 * 1024;
    await emit(EVENTS.modelsProgress, [
      { id: "vad", name: "Voice detection", bytes_total: 2 * mb, bytes_done: 2 * mb, state: "ready" },
      { id: "asr", name: "Speech recognition", bytes_total: 239 * mb, bytes_done: 239 * mb, state: "verifying" },
      { id: "mt", name: "Translation", bytes_total: 1105 * mb, bytes_done: 1105 * mb, state: "corrupt" },
    ]);
    await settle();
    const rows = [...root.querySelectorAll(".model")].map((row) => text(row));
    expect(rows[1]).toBe("Speech recognition 239 MB Checking file…");
    expect(rows[2]).toBe("Translation 1.08 GB Damaged Retry");
    byRole(root, "button", "Retry").click();
    await settle();
    expect(backend.last("models_download")?.args).toEqual({ source: "huggingface" });
  });

  it("switches to Capture and starts listening exactly once when every model is ready", async () => {
    const { root, backend } = await open(firstRun);
    const mb = 1024 * 1024;
    const ready = [
      { id: "vad", name: "Voice detection", bytes_total: 2 * mb, bytes_done: 2 * mb, state: "ready" as const },
      { id: "asr", name: "Speech recognition", bytes_total: 239 * mb, bytes_done: 239 * mb, state: "ready" as const },
      { id: "mt", name: "Translation", bytes_total: 1105 * mb, bytes_done: 1105 * mb, state: "ready" as const },
    ];
    await emit(EVENTS.modelsProgress, ready);
    await settle();
    expect(backend.count("start_listening")).toBe(1);
    expect(tab(root, "Capture").getAttribute("aria-selected")).toBe("true");
    await emit(EVENTS.modelsProgress, ready);
    await emit(EVENTS.modelsProgress, ready);
    await settle();
    expect(backend.count("start_listening")).toBe(1);
  });

  it("reports a failed start without looping", async () => {
    const { root, backend } = await open(firstRun);
    backend.failures.set("start_listening", "The audio device is busy.");
    await emit(
      EVENTS.modelsProgress,
      READY_MODELS,
    );
    await settle();
    expect(backend.count("start_listening")).toBe(1);
    expect(text(q(root, '[role="alert"]'))).toContain("The audio device is busy.");
  });

  it("uses an existing folder chosen through the dialog and then starts once", async () => {
    const { root, backend } = await open(firstRun);
    byRole(root, "button", "Use files I already have").click();
    await settle();
    expect(backend.last("plugin:dialog|open")?.args).toMatchObject({ options: { directory: true, multiple: false } });
    expect(backend.last("models_use_existing")?.args).toEqual({ folder: "D:\\existing-models" });
    expect(backend.count("start_listening")).toBe(1);
    expect(tab(root, "Capture").getAttribute("aria-selected")).toBe("true");
  });

  it("does nothing when the folder dialog is cancelled", async () => {
    const { root, backend } = await open((b) => {
      firstRun(b);
      b.dialogResult = null;
    });
    byRole(root, "button", "Use files I already have").click();
    await settle();
    expect(backend.count("models_use_existing")).toBe(0);
    expect(backend.count("start_listening")).toBe(0);
  });

  it("shows download errors from the mirror in plain words", async () => {
    const { root, backend } = await open(firstRun);
    backend.failures.set("models_download", "Hugging Face could not be reached. Try ModelScope.");
    byRole(root, "button", "Download").click();
    await settle();
    expect(text(q(root, '[role="alert"]'))).toContain("Hugging Face could not be reached. Try ModelScope.");
  });

  it("does not show first run at all when every model is ready, and never starts by itself", async () => {
    const { root, backend } = await open();
    expect(root.querySelector("h1")).toBeNull();
    expect(backend.count("start_listening")).toBe(0);
  });
});
