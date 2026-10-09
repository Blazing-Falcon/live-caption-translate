// @vitest-environment jsdom
import { emit } from "@tauri-apps/api/event";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { createSession, EVENTS, type Session } from "../src/lib/events";
import ControlApp from "../src/control/ControlApp.svelte";
import { DRAFT_MODEL, FakeBackend, MISSING_MODELS, READY_MODELS } from "./backend";
import { accessibleName, byRole, cleanup, q, render, setValue, settle, text } from "./dom";
import { FakeClock, STATS, testState } from "./helpers";

const sessions: Session[] = [];

async function open(configure?: (backend: FakeBackend) => void) {
  const backend = new FakeBackend();
  configure?.(backend);
  backend.install();
  const session = createSession({ kind: "control", clock: new FakeClock(), reducedMotion: () => false });
  sessions.push(session);
  const root = render(ControlApp, { session });
  await session.ready;
  await settle();
  return { backend, session, root };
}

async function go(root: HTMLElement, name: string): Promise<void> {
  byRole(root, "tab", name).click();
  await settle();
}

afterEach(() => {
  sessions.splice(0).forEach((session) => session.dispose());
  cleanup();
  clearMocks();
});

describe("Performance page: stats", () => {
  it("shows word to first and final English with the median note, then queue, memory and CPU", async () => {
    const { root } = await open((b) => {
      b.stats = { ...STATS, word_first_p50_ms: 1040, word_final_p50_ms: 2870 };
    });
    await go(root, "Performance");
    expect(text(q(root, ".stat-grid"))).toBe(
      "Word to first English 1.0 s median, last minute Word to final English 2.9 s median, last minute Waiting to translate 1 skipped this session: 3 Memory 1.9 GB CPU 41%, app and translator",
    );
    expect(root.textContent).not.toContain("Speech end to English");
  });

  it("counts the draft server in memory and CPU", async () => {
    const { root } = await open((b) => {
      b.stats = { ...STATS, mode: "continuous", cpu_draft_pct: 55, rss_draft_mb: 610 };
    });
    await go(root, "Performance");
    expect(text(q(root, '[data-testid="stat-memory"]'))).toBe("2.5 GB");
    expect(text(q(root, ".stat-grid"))).toContain("CPU 96%, app and translators");
    expect(text(q(root, '[data-testid="footer"]'))).toContain("CPU 96%");
  });

  it("shows a dash for word delays that are not measured yet", async () => {
    const { root } = await open();
    await go(root, "Performance");
    expect(text(q(root, '[data-testid="stat-first"]'))).toBe("—");
    expect(text(q(root, '[data-testid="stat-final"]'))).toBe("—");
  });
});

describe("Performance page: caption speed", () => {
  const SPEEDS = [
    ["auto", "Automatic (recommended)", "Uses Continuous on PCs with 6 or more cores, Light on smaller PCs."],
    ["continuous", "Continuous", "English follows the speaker within about a second. Uses the most CPU."],
    ["light", "Light", "Translates each phrase as soon as it ends. Less CPU, no draft text."],
    ["off", "Wait for full sentences", "Translates after the speaker pauses, like version 1."],
  ] as const;
  const speed = (root: HTMLElement) => byRole(root, "combobox", "Caption speed") as HTMLSelectElement;

  it("is the first control on the page, with the exact option labels and Automatic selected", async () => {
    const { root } = await open();
    await go(root, "Performance");
    const select = speed(root);
    expect([...select.options].map((option) => [option.value, option.textContent?.trim()])).toEqual(
      SPEEDS.map(([value, label]) => [value, label]),
    );
    expect(select.value).toBe("auto");
    expect(q(root, "section.page select")).toBe(select);
  });

  it.each(SPEEDS)("saves latency.mode = %s and shows its help text under the select", async (value, _label, help) => {
    const { root, backend } = await open((b) => (b.config.latency.mode = value === "auto" ? "light" : "auto"));
    await go(root, "Performance");
    setValue(speed(root), value, ["change"]);
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { latency: { mode: value } } });
    expect(text(q(root, '[data-testid="caption-speed-help"]'))).toBe(help);
    expect(speed(root).getAttribute("aria-describedby")).toBe("caption-speed-help");
  });

  it("shows the help text of the saved mode on load and puts the status line below it", async () => {
    const { root } = await open((b) => (b.config.latency.mode = "off"));
    await go(root, "Performance");
    const help = q(root, '[data-testid="caption-speed-help"]');
    expect(text(help)).toBe("Translates after the speaker pauses, like version 1.");
    expect(help.nextElementSibling).toBe(q(root, ".status-row"));
  });

  it("reverts the select when saving fails and shows the restart notice the backend asks for", async () => {
    const { root, backend } = await open();
    await go(root, "Performance");
    backend.failures.set("set_config", "Could not save settings.");
    setValue(speed(root), "off", ["change"]);
    await settle();
    expect(speed(root).value).toBe("auto");
    expect(text(q(root, '[role="alert"]'))).toContain("Could not save settings.");
    backend.failures.delete("set_config");
    backend.applied = "pipeline";
    setValue(speed(root), "off", ["change"]);
    await settle();
    expect(speed(root).value).toBe("off");
    expect(text(q(root, '[role="status"]'))).toContain("Speech recognition and translation restart briefly.");
  });

  it.each<[string, string | null, string]>([
    ["continuous", null, "Now: Continuous"],
    ["continuous", "auto", "Now: Continuous"],
    ["light", null, "Now: Light"],
    ["light", "user", "Now: Light"],
    ["light", "auto", "Now: Light"],
    ["light", "cpu", "Now: Light, because the PC is busy"],
    ["light", "lag", "Now: Light, because translation fell behind"],
    ["light", "draft_unavailable", "Now: Light, because the draft model is not downloaded"],
    ["off", null, "Now: Off"],
    ["off", "user", "Now: Off"],
  ])("status line for mode %s and reason %s reads %s", async (mode, reason, expected) => {
    const { root } = await open((b) => {
      b.stats = { ...STATS, mode: mode as "light", mode_reason: reason as "cpu" | null };
    });
    await go(root, "Performance");
    expect(text(q(root, '[data-testid="mode-status"]'))).toBe(expected);
  });

  it("uses the state snapshot before the first stats and follows stats events afterwards", async () => {
    const { root } = await open((b) => {
      b.failures.set("get_stats", "No stats yet.");
      b.state = testState({ mode: "continuous", mode_reason: "auto" });
    });
    await go(root, "Performance");
    expect(text(q(root, '[data-testid="mode-status"]'))).toBe("Now: Continuous");
    await emit(EVENTS.pipeline, { type: "stats", ...STATS, mode: "light", mode_reason: "cpu" });
    await settle();
    expect(text(q(root, '[data-testid="mode-status"]'))).toBe("Now: Light, because the PC is busy");
    await emit(EVENTS.pipeline, { type: "stats", ...STATS, mode: "continuous", mode_reason: null });
    await settle();
    expect(text(q(root, '[data-testid="mode-status"]'))).toBe("Now: Continuous");
  });

  const downloadButton = (root: HTMLElement): HTMLElement | null =>
    [...root.querySelectorAll<HTMLElement>("button")].find((button) => text(button) === "Download (480 MB)") ?? null;

  it("offers Download (480 MB) when the draft model is missing and downloads exactly the draft model", async () => {
    const { root, backend } = await open((b) => {
      b.stats = { ...STATS, mode: "light", mode_reason: "draft_unavailable" };
      b.models = [...structuredClone(READY_MODELS), { ...DRAFT_MODEL }];
      b.config.models.source = "modelscope";
    });
    await go(root, "Performance");
    const button = downloadButton(root);
    expect(button).not.toBeNull();
    button?.click();
    await settle();
    expect(backend.last("models_download")?.args).toEqual({ source: "modelscope", ids: ["lmt-60-0.6b-q4_k_m"] });
    expect(backend.count("models_download")).toBe(1);
  });

  it.each<[string, string | null]>([
    ["continuous", null],
    ["continuous", "auto"],
    ["light", null],
    ["light", "cpu"],
    ["light", "lag"],
    ["light", "user"],
    ["off", null],
  ])("shows no Download button for mode %s, reason %s", async (mode, reason) => {
    const { root } = await open((b) => {
      b.stats = { ...STATS, mode: mode as "light", mode_reason: reason as "cpu" | null };
    });
    await go(root, "Performance");
    expect(downloadButton(root)).toBeNull();
  });

  it("does not start a second draft download while one is running, and reports a failed start", async () => {
    const { root, backend } = await open((b) => {
      b.stats = { ...STATS, mode: "light", mode_reason: "draft_unavailable" };
      b.models = [...structuredClone(READY_MODELS), { ...DRAFT_MODEL, state: "downloading", bytes_done: 100 }];
    });
    await go(root, "Performance");
    downloadButton(root)?.click();
    await settle();
    expect(backend.count("models_download")).toBe(0);
    expect(downloadButton(root)?.getAttribute("aria-disabled")).toBe("true");
    await emit(EVENTS.modelsProgress, [...READY_MODELS, { ...DRAFT_MODEL, state: "missing", bytes_done: 0 }]);
    await settle();
    backend.failures.set("models_download", "Hugging Face could not be reached. Try ModelScope.");
    downloadButton(root)?.click();
    await settle();
    expect(text(q(root, '[role="alert"]'))).toContain("Hugging Face could not be reached.");
  });
});

describe("Performance page: priority, step-down and long sentences", () => {
  const box = (root: HTMLElement, name: string) => byRole(root, "checkbox", name) as HTMLInputElement;
  const ROWS = [
    ["Give games priority", "low_priority", "Runs translation at low priority so games stay smooth. Off: English can appear slightly sooner, but uses more CPU."],
    ["Slow down when the PC is busy", "step_down", "Switches to Light while the PC is busy and back when it is free."],
    ["Split long sentences", "split_long", "Translates long sentences in parts when the speaker does not pause."],
  ] as const;

  it("shows the three checkboxes on by default with their one-line help text", async () => {
    const { root } = await open();
    await go(root, "Performance");
    for (const [label, , help] of ROWS) {
      const input = box(root, label);
      expect(input.checked, label).toBe(true);
      expect(input.disabled, label).toBe(false);
      const described = input.getAttribute("aria-describedby") ?? "";
      expect(text(q(root, `#${described}`)), label).toBe(help);
    }
  });

  it.each(ROWS)("%s saves latency.%s and shows the restart notice when the backend asks for it", async (label, key) => {
    const { root, backend } = await open();
    await go(root, "Performance");
    backend.applied = "pipeline";
    box(root, label).click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { latency: { [key]: false } } });
    expect(box(root, label).checked).toBe(false);
    expect(text(q(root, '[role="status"]'))).toContain("Speech recognition and translation restart briefly.");
    box(root, label).click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { latency: { [key]: true } } });
  });

  it("reverts a checkbox when saving fails", async () => {
    const { root, backend } = await open();
    await go(root, "Performance");
    backend.failures.set("set_config", "Could not save settings.");
    box(root, "Give games priority").click();
    await settle();
    expect(box(root, "Give games priority").checked).toBe(true);
  });

  it.each<[string, boolean, boolean]>([
    ["auto", false, false],
    ["continuous", false, false],
    ["light", true, false],
    ["off", true, true],
  ])("with Caption speed %s: Slow down disabled=%s, Split long sentences disabled=%s", async (mode, stepDownOff, splitOff) => {
    const { root } = await open((b) => (b.config.latency.mode = mode as "auto"));
    await go(root, "Performance");
    expect(box(root, "Slow down when the PC is busy").disabled).toBe(stepDownOff);
    expect(box(root, "Split long sentences").disabled).toBe(splitOff);
    expect(box(root, "Give games priority").disabled).toBe(false);
  });

  it("keeps disabled rows readable and enables them again when the speed changes", async () => {
    const { root } = await open((b) => (b.config.latency.mode = "off"));
    await go(root, "Performance");
    expect(text(q(root, "#help-step-down"))).toBe("Switches to Light while the PC is busy and back when it is free.");
    expect(text(box(root, "Slow down when the PC is busy").closest("label") as HTMLElement)).toBe("Slow down when the PC is busy");
    setValue(byRole(root, "combobox", "Caption speed") as HTMLSelectElement, "continuous", ["change"]);
    await settle();
    expect(box(root, "Slow down when the PC is busy").disabled).toBe(false);
    expect(box(root, "Split long sentences").disabled).toBe(false);
  });

  it("does not save when a disabled checkbox is clicked", async () => {
    const { root, backend } = await open((b) => (b.config.latency.mode = "off"));
    await go(root, "Performance");
    box(root, "Split long sentences").click();
    await settle();
    expect(backend.count("set_config")).toBe(0);
  });

  it("keeps Translator threads and shows no other [latency] tuning key in the control window", async () => {
    const { root } = await open();
    let body = "";
    for (const name of ["Capture", "Overlay", "Languages", "Performance"]) {
      await go(root, name);
      body += ` ${text(root)}`;
    }
    expect(byRole(root, "combobox", "Translator threads")).toBeTruthy();
    for (const hidden of ["comma", "decode interval", "cap_tokens", "step_down_cpu", "draft_timeout", "auto_min_cores"]) {
      expect(body.toLowerCase()).not.toContain(hidden);
    }
  });
});

describe("Overlay page: live Chinese and draft text", () => {
  it("has a Show Chinese while someone is speaking checkbox, on by default, saving overlay.live_source", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    const input = byRole(root, "checkbox", "Show Chinese while someone is speaking") as HTMLInputElement;
    expect(input.checked).toBe(true);
    input.click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { live_source: false } } });
    expect(input.checked).toBe(false);
    expect(root.querySelector('[role="status"]')).toBeNull();
    input.click();
    await settle();
    expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { live_source: true } } });
  });

  it("has a Draft text select with the three policies and the help text under it", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    const select = byRole(root, "combobox", "Draft text") as HTMLSelectElement;
    expect([...select.options].map((option) => [option.value, option.textContent?.trim()])).toEqual([
      ["hold2", "Hold back the newest words"],
      ["settled", "Show only settled words"],
      ["all", "Show everything"],
    ]);
    expect(select.value).toBe("hold2");
    expect(text(q(root, "#draft-display-help"))).toBe(
      "Drafts are quick translations shown in a dimmer color until the final translation replaces them.",
    );
    expect(select.getAttribute("aria-describedby")).toBe("draft-display-help");
    for (const value of ["settled", "all", "hold2"]) {
      setValue(select, value, ["change"]);
      await settle();
      expect(backend.last("set_config")?.args).toEqual({ patch: { overlay: { draft_display: value } } });
    }
    expect(root.querySelector('[role="status"]')).toBeNull();
  });

  it("reverts the select and the checkbox when saving fails", async () => {
    const { root, backend } = await open();
    await go(root, "Overlay");
    backend.failures.set("set_config", "Could not save settings.");
    const select = byRole(root, "combobox", "Draft text") as HTMLSelectElement;
    setValue(select, "all", ["change"]);
    const input = byRole(root, "checkbox", "Show Chinese while someone is speaking") as HTMLInputElement;
    input.click();
    await settle();
    expect(select.value).toBe("hold2");
    expect(input.checked).toBe(true);
  });

  it("reads the saved values, and the live settings change the open overlay without a restart notice", async () => {
    const { root } = await open((b) => {
      b.config.overlay.live_source = false;
      b.config.overlay.draft_display = "settled";
    });
    await go(root, "Overlay");
    expect((byRole(root, "checkbox", "Show Chinese while someone is speaking") as HTMLInputElement).checked).toBe(false);
    expect((byRole(root, "combobox", "Draft text") as HTMLSelectElement).value).toBe("settled");
    expect([...root.querySelectorAll("select")].map(accessibleName)).toEqual(["Draft text"]);
  });
});

describe("First run: the optional draft model", () => {
  const firstRun = (draft: Partial<typeof DRAFT_MODEL>) => (b: FakeBackend) => {
    b.models = [...structuredClone(MISSING_MODELS), { ...DRAFT_MODEL, ...draft }];
    b.state = testState({ models_ready: false, listening: "paused" });
  };
  const rows = (root: HTMLElement) => [...root.querySelectorAll(".model")].map((row) => text(row));

  it("lists the draft model as Faster captions (LMT-60 0.6B, 480 MB)", async () => {
    const { root } = await open(firstRun({ recommended: true }));
    expect(rows(root)).toEqual([
      "Voice detection 2.0 MB Done",
      "Speech recognition 239 MB Waiting",
      "Translation 1.08 GB Waiting",
      "Faster captions (LMT-60 0.6B, 480 MB) Waiting",
    ]);
  });

  it("includes a recommended draft model in Download without spelling out ids, and counts its size", async () => {
    const { root, backend } = await open(firstRun({ recommended: true }));
    expect(root.querySelector(".model input[type=checkbox]")).toBeNull();
    expect(text(q(root, ".intro p"))).toContain("about 1.77 GB");
    byRole(root, "button", "Download").click();
    await settle();
    expect(backend.last("models_download")?.args).toEqual({ source: "huggingface" });
  });

  it("lists it as optional on smaller PCs and does not download it unless ticked", async () => {
    const { root, backend } = await open(firstRun({ recommended: false }));
    const box = byRole(root, "checkbox", "Download Faster captions (LMT-60 0.6B, 480 MB)") as HTMLInputElement;
    expect(box.checked).toBe(false);
    expect(rows(root)[3]).toBe("Faster captions (LMT-60 0.6B, 480 MB) Optional");
    expect(text(q(root, ".intro p"))).toContain("about 1.32 GB");
    byRole(root, "button", "Download").click();
    await settle();
    expect(backend.last("models_download")?.args).toEqual({ source: "huggingface" });
  });

  it("downloads exactly the missing required models plus the ticked optional one", async () => {
    const { root, backend } = await open(firstRun({ recommended: false }));
    const box = byRole(root, "checkbox", "Download Faster captions (LMT-60 0.6B, 480 MB)") as HTMLInputElement;
    box.click();
    await settle();
    expect(box.checked).toBe(true);
    expect(rows(root)[3]).toBe("Faster captions (LMT-60 0.6B, 480 MB) Waiting");
    expect(text(q(root, ".intro p"))).toContain("about 1.77 GB");
    byRole(root, "button", "Download").click();
    await settle();
    expect(backend.last("models_download")?.args).toEqual({
      source: "huggingface",
      ids: ["asr", "mt", "lmt-60-0.6b-q4_k_m"],
    });
    box.click();
    await settle();
    byRole(root, "button", "Download").click();
    await settle();
    expect(backend.last("models_download")?.args).toEqual({ source: "huggingface" });
  });

  it("starts listening once the required models are ready even though the draft model is missing", async () => {
    const { root, backend } = await open(firstRun({ recommended: true }));
    await emit(EVENTS.modelsProgress, [...READY_MODELS, { ...DRAFT_MODEL, recommended: true }]);
    await settle();
    expect(backend.count("start_listening")).toBe(1);
    expect(byRole(root, "tab", "Capture").getAttribute("aria-selected")).toBe("true");
  });

  it("never shows first run for a PC that only lacks the optional draft model", async () => {
    const { root, backend } = await open((b) => {
      b.models = [...structuredClone(READY_MODELS), { ...DRAFT_MODEL }];
    });
    expect(root.querySelector("h1")).toBeNull();
    expect(backend.count("start_listening")).toBe(0);
  });
});
