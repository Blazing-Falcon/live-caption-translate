// @vitest-environment jsdom
import { emit } from "@tauri-apps/api/event";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { createSession, EVENTS, type Session } from "../src/lib/events";
import type { CaptionLine as Line } from "../src/lib/types";
import CaptionLine from "../src/overlay/CaptionLine.svelte";
import CaptionPanel from "../src/overlay/CaptionPanel.svelte";
import MoveFrame from "../src/overlay/MoveFrame.svelte";
import OverlayApp from "../src/overlay/OverlayApp.svelte";
import StatusChip from "../src/overlay/StatusChip.svelte";
import SubtitleBar from "../src/overlay/SubtitleBar.svelte";
import { FakeBackend } from "./backend";
import { byRole, cleanup, q, render, settle, text } from "./dom";
import { FakeClock, asrFinal, delta, final, testState } from "./helpers";

const line = (patch: Partial<Line> & Pick<Line, "id" | "state">): Line => ({
  source: "你别误会，我不是那个意思。",
  english: "",
  lang: "zh",
  reason: null,
  updatedAt: 0,
  ...patch,
});

afterEach(() => {
  cleanup();
  clearMocks();
});

describe("CaptionLine states", () => {
  it("pending shows the source and a dim ellipsis", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "pending" }) });
    expect(q(root, ".source").textContent).toBe("你别误会，我不是那个意思。");
    expect(q(root, ".dots").textContent).toBe("···");
    expect(root.querySelector(".caret")).toBeNull();
    expect(text(root)).toContain("Translating");
  });

  it("keeps the source in pending, skipped and failed states even when show_source is off", () => {
    for (const state of ["pending", "skipped", "failed"] as const) {
      const root = render(CaptionLine, { line: line({ id: 1, state, reason: "x" }), showSource: false });
      expect(q(root, ".source").textContent).toBe("你别误会，我不是那个意思。");
    }
  });

  it("streaming shows the English so far with a caret", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "streaming", english: "Don't get it wrong, that's" }) });
    expect(q(root, ".english").textContent).toBe("Don't get it wrong, that's");
    expect(root.querySelector(".english .caret")).not.toBeNull();
    expect(q(root, ".source").textContent).toContain("你别误会");
  });

  it("final shows English without a caret and obeys show_source", () => {
    const withSource = render(CaptionLine, { line: line({ id: 1, state: "final", english: "That's not what I mean." }) });
    expect(withSource.querySelector(".caret")).toBeNull();
    expect(withSource.querySelector(".source")).not.toBeNull();
    const without = render(CaptionLine, {
      line: line({ id: 2, state: "final", english: "That's not what I mean." }),
      showSource: false,
    });
    expect(without.querySelector(".source")).toBeNull();
    expect(q(without, ".english").textContent).toBe("That's not what I mean.");
  });

  it("streaming also hides the source when show_source is off", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "streaming", english: "So" }), showSource: false });
    expect(root.querySelector(".source")).toBeNull();
    expect(root.querySelector(".caret")).not.toBeNull();
  });

  it("English speech is shown as heard with its own label and no source line", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "english", source: "OK, can you guys hear me now?", lang: "en" }) });
    expect(q(root, ".main").textContent).toBe("OK, can you guys hear me now?");
    expect(q(root, ".label").textContent).toBe("English · Not translated");
    expect(root.querySelector(".source")).toBeNull();
  });

  it("Other language says how to turn translation on for Japanese and Korean only", () => {
    const ja = render(CaptionLine, { line: line({ id: 1, state: "other", source: "みんな、ありがとう！", lang: "ja" }) });
    expect(q(ja, ".label").textContent).toBe("Japanese · Turn on in settings to translate");
    expect(q(ja, ".main").getAttribute("lang")).toBe("ja");
    const ko = render(CaptionLine, { line: line({ id: 2, state: "other", source: "감사합니다", lang: "ko" }) });
    expect(q(ko, ".label").textContent).toBe("Korean · Turn on in settings to translate");
    const de = render(CaptionLine, { line: line({ id: 3, state: "other", source: "Danke", lang: "de" }) });
    expect(q(de, ".label").textContent).toBe("De · Not translated");
    const none = render(CaptionLine, { line: line({ id: 4, state: "other", source: "?", lang: null }) });
    expect(q(none, ".label").textContent).toBe("Other language · Not translated");
  });

  it("skipped keeps the source and says it was skipped", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "skipped" }) });
    expect(q(root, "article").classList.contains("skipped")).toBe(true);
    expect(q(root, ".label").textContent).toBe("Skipped to catch up");
    expect(q(root, ".source").textContent).toContain("你别误会");
  });

  it("failed shows the mapped reason in the warning style", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "failed", reason: "Translator is restarting" }) });
    const label = q(root, ".label");
    expect(label.textContent).toBe("Translation failed · Translator is restarting");
    expect(label.classList.contains("warning")).toBe(true);
  });

  it("status replaces the caption with a hollow-dot line", () => {
    const root = render(CaptionLine, { status: { kind: "no_device", text: "No audio device" } });
    expect(q(root, ".status").textContent).toContain("No audio device");
    expect(q(root, ".hollow").getAttribute("aria-hidden")).toBe("true");
    expect(root.querySelector("article")).toBeNull();
  });

  it("applies age opacity, older scale and fading", () => {
    const newest = render(CaptionLine, { line: line({ id: 1, state: "final" }), variant: "bar", age: 0 });
    expect(q(newest, "article").style.getPropertyValue("--age-opacity")).toBe("1");
    const older = render(CaptionLine, { line: line({ id: 2, state: "final" }), variant: "bar", age: 1, fading: true });
    expect(q(older, "article").style.getPropertyValue("--age-opacity")).toBe("0.55");
    expect(q(older, "article").classList.contains("older")).toBe(true);
    expect(q(older, "article").classList.contains("fading")).toBe(true);
    const panel = render(CaptionLine, { line: line({ id: 3, state: "final" }), variant: "panel", age: 3 });
    expect(q(panel, "article").style.getPropertyValue("--age-opacity")).toBe("0.45");
  });
});

describe("SubtitleBar", () => {
  const three = [
    line({ id: 1, state: "final", english: "One" }),
    line({ id: 2, state: "final", english: "Two" }),
    line({ id: 3, state: "streaming", english: "Thr" }),
  ];

  it("renders only the last two utterances, older one faded", () => {
    const root = render(SubtitleBar, { lines: three });
    const articles = [...root.querySelectorAll("article")];
    expect(articles.map((a) => a.getAttribute("data-id"))).toEqual(["2", "3"]);
    expect(articles[0]?.classList.contains("older")).toBe(true);
    expect(articles[1]?.classList.contains("older")).toBe(false);
  });

  it("renders nothing when there is nothing to say and a status line instead of captions", () => {
    expect(render(SubtitleBar, { lines: [] }).querySelector("section")).toBeNull();
    const root = render(SubtitleBar, { lines: three, status: { kind: "paused", text: "Paused · Ctrl Shift P to resume" } });
    expect(root.querySelector("article")).toBeNull();
    expect(text(root)).toContain("Paused · Ctrl Shift P to resume");
  });

  it("marks lines that are fading", () => {
    const root = render(SubtitleBar, { lines: three, fadingIds: new Set([2]) });
    expect(q(root, '[data-id="2"]').classList.contains("fading")).toBe(true);
    expect(q(root, '[data-id="3"]').classList.contains("fading")).toBe(false);
  });
});

describe("CaptionPanel", () => {
  const many = Array.from({ length: 8 }, (_, index) =>
    line({ id: index + 1, state: "final", english: `Line ${index + 1}` }),
  );

  it("renders panel_lines utterances, newest last", () => {
    expect(
      [...render(CaptionPanel, { lines: many, panelLines: 5 }).querySelectorAll("article")].map((a) => a.getAttribute("data-id")),
    ).toEqual(["4", "5", "6", "7", "8"]);
    expect(render(CaptionPanel, { lines: many, panelLines: 4 }).querySelectorAll("article")).toHaveLength(4);
    expect(render(CaptionPanel, { lines: many, panelLines: 6 }).querySelectorAll("article")).toHaveLength(6);
  });

  it("has a hover header with labelled pause and settings buttons", () => {
    const calls: string[] = [];
    const root = render(CaptionPanel, {
      lines: [],
      headerText: "Listening · Chrome, VLC",
      onpause: () => calls.push("pause"),
      onsettings: () => calls.push("settings"),
    });
    expect(text(q(root, '[data-testid="panel-header"]'))).toBe("Listening · Chrome, VLC");
    byRole(root, "button", "Pause").click();
    byRole(root, "button", "Open settings").click();
    expect(calls).toEqual(["pause", "settings"]);
  });

  it("offers Resume while paused and can be revealed without hover", () => {
    const root = render(CaptionPanel, { lines: [], listening: false, reveal: true });
    expect(byRole(root, "button", "Resume")).toBeTruthy();
    expect(q(root, "section").classList.contains("reveal")).toBe(true);
  });
});

describe("StatusChip and MoveFrame", () => {
  it("shows the chip as a status region and nothing without one", () => {
    expect(render(StatusChip, { chip: null }).querySelector(".chip")).toBeNull();
    const root = render(StatusChip, { chip: { id: 1, text: "Listening · Speakers" } });
    expect(q(root, '[role="status"]').textContent).toContain("Listening · Speakers");
  });

  it("move frame shows the hotkey and a Lock button", () => {
    let locked = 0;
    const root = render(MoveFrame, { binding: "Ctrl Shift L", onlock: () => (locked += 1) });
    expect(text(root)).toContain("Move or resize, then lock");
    expect(q(root, "kbd").textContent).toBe("Ctrl Shift L");
    byRole(root, "button", "Lock").click();
    expect(locked).toBe(1);
  });
});

describe("OverlayApp", () => {
  const sessions: Session[] = [];

  async function open(configure?: (backend: FakeBackend) => void) {
    const backend = new FakeBackend();
    configure?.(backend);
    backend.install();
    const clock = new FakeClock();
    const session = createSession({ kind: "overlay", clock, reducedMotion: () => false });
    sessions.push(session);
    const root = render(OverlayApp, { session });
    await session.ready;
    await settle();
    return { backend, clock, session, root };
  }

  afterEach(() => sessions.splice(0).forEach((session) => session.dispose()));

  it("renders a streamed utterance in the subtitle bar and applies size and opacity tokens", async () => {
    const { root } = await open((b) => {
      b.config.overlay.font_px = 32;
      b.config.overlay.background = 0.6;
    });
    await emit(EVENTS.pipeline, asrFinal(1, "大家好。"));
    await settle();
    expect(q(root, '[data-state="pending"]')).toBeTruthy();
    await emit(EVENTS.pipeline, delta(1, "Hello everyone"));
    await settle();
    expect(q(root, ".english").textContent).toBe("Hello everyone");
    expect(root.querySelector(".caret")).not.toBeNull();
    await emit(EVENTS.pipeline, final(1, "Hello everyone."));
    await settle();
    expect(root.querySelector(".caret")).toBeNull();
    const main = q(root, "main");
    expect(main.style.getPropertyValue("--font-px")).toBe("32");
    expect(main.style.getPropertyValue("--ov-bar-alpha")).toBe("0.6");
    expect(root.querySelector('[data-testid="subtitle-bar"]')).not.toBeNull();
  });

  it("switches to the caption panel from config and announces finished lines politely", async () => {
    const { root } = await open((b) => (b.config.overlay.style = "panel"));
    expect(root.querySelector('[data-testid="caption-panel"]')).not.toBeNull();
    await emit(EVENTS.pipeline, asrFinal(1, "你好。"));
    await emit(EVENTS.pipeline, final(1, "Hello."));
    await settle();
    const live = q(root, '[aria-live="polite"]');
    expect(live.textContent).toBe("Hello.");
  });

  it("replaces captions with the paused status using the configured binding, then shows a chip on resume", async () => {
    const { root } = await open((b) => (b.config.hotkeys.pause = "Ctrl+Alt+P"));
    await emit(EVENTS.pipeline, { type: "listening_state", state: "paused" });
    await settle();
    expect(text(q(root, ".status"))).toBe("Paused · Ctrl Alt P to resume");
    await emit(EVENTS.pipeline, { type: "listening_state", state: "listening" });
    await settle();
    expect(root.querySelector(".status")).toBeNull();
    expect(q(root, '[data-testid="status-chip"]').textContent).toContain("Listening");
  });

  it("shows the move frame and locks through set_overlay_moving(false)", async () => {
    const { root, backend } = await open((b) => (b.state = testState({ overlay_moving: true })));
    expect(root.querySelector('[data-testid="move-frame"]')).not.toBeNull();
    byRole(root, "button", "Lock").click();
    await settle();
    expect(backend.last("set_overlay_moving")?.args).toEqual({ moving: false });
    await emit(EVENTS.overlayMode, { moving: false });
    await settle();
    expect(root.querySelector('[data-testid="move-frame"]')).toBeNull();
  });

  it("wires the panel header buttons to pause and show the control window", async () => {
    const { root, backend } = await open((b) => (b.config.overlay.style = "panel"));
    byRole(root, "button", "Pause").click();
    byRole(root, "button", "Open settings").click();
    await settle();
    expect(backend.count("pause_listening")).toBe(1);
    expect(backend.count("show_control_window")).toBe(1);
  });

  it("renders nothing until the config has loaded", async () => {
    const backend = new FakeBackend();
    backend.failures.set("get_config", "The settings are unavailable.");
    backend.install();
    const session = createSession({ kind: "overlay", clock: new FakeClock(), reducedMotion: () => false });
    sessions.push(session);
    const root = render(OverlayApp, { session });
    await session.ready;
    await settle();
    expect(root.querySelector("main")).toBeNull();
  });
});
