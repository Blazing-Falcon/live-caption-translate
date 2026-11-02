import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { OCR_MODEL_ID } from "../src/lib/screen";
import { DEFAULT_CONFIG, HOTKEY_ACTIONS, applyPatch, recordShortcut } from "../src/lib/settings";

function parseDefaultToml(): Record<string, unknown> {
  const doc = readFileSync(new URL("../../../docs/configuration.md", import.meta.url), "utf8");
  const block = /## Full Default File\s+```toml\r?\n([\s\S]*?)```/.exec(doc)?.[1];
  if (!block) throw new Error("default config block not found in docs/configuration.md");
  const root: Record<string, unknown> = {};
  let table: Record<string, unknown> = root;
  for (const raw of block.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    const header = /^\[([a-z_.]+)\]/.exec(line);
    if (header?.[1]) {
      table = root;
      for (const part of header[1].split(".")) {
        table[part] ??= {};
        table = table[part] as Record<string, unknown>;
      }
      continue;
    }
    const pair = /^([a-z_]+)\s*=\s*(.*)$/.exec(line);
    if (!pair?.[1] || pair[2] === undefined) throw new Error(`unparsable line: ${line}`);
    let value = pair[2];
    const quoted = /^("(?:[^"\\]|\\.)*"|\[[^\]]*\])/.exec(value);
    value = quoted?.[1] ?? value.replace(/\s+#.*$/, "").trim();
    table[pair[1]] = JSON.parse(value) as unknown;
  }
  return root;
}

describe("DEFAULT_CONFIG", () => {

  it("matches the full default file in docs/configuration.md", () => {
    expect(parseDefaultToml()).toEqual(DEFAULT_CONFIG);
  });

  it("stays the last statement of settings.ts as plain JSON, which is how the Rust parity test reads it", () => {
    const source = readFileSync(new URL("../src/lib/settings.ts", import.meta.url), "utf8");
    const initializer = source.split("export const DEFAULT_CONFIG: Config = ")[1]?.trim().replace(/;$/, "");
    expect(initializer).toBeDefined();
    expect(JSON.parse(initializer ?? "")).toEqual(DEFAULT_CONFIG);
    expect(source.split("export const DEFAULT_CONFIG").length).toBe(2);
  });

  it("has a shortcut field in the control window for every hotkey", () => {
    expect([...HOTKEY_ACTIONS].sort()).toEqual(Object.keys(DEFAULT_CONFIG.hotkeys).sort());
  });

});


describe("OCR_MODEL_ID", () => {
  it("names the optional model in models/manifest.json", () => {
    const manifest = JSON.parse(readFileSync(new URL("../../../models/manifest.json", import.meta.url), "utf8")) as {
      models: { id: string; optional?: boolean }[];
    };
    expect(manifest.models.find((model) => model.id === OCR_MODEL_ID)?.optional).toBe(true);
  });
});


describe("applyPatch", () => {
  it("merges nested tables, replaces arrays and removes keys set to null", () => {
    const base = structuredClone(DEFAULT_CONFIG);
    base.overlay.panel_rect = { monitor: "A", x: 1, y: 2, w: 3, h: 4 };
    const next = applyPatch(base, {
      overlay: { font_px: 30, panel_rect: null },
      capture: { apps: [{ exe: "chrome.exe", name: "Chrome" }] },
    });
    expect(next.overlay.font_px).toBe(30);
    expect(next.overlay.style).toBe(DEFAULT_CONFIG.overlay.style);
    expect(next.overlay.panel_rect).toBeUndefined();
    expect(next.capture.apps).toEqual([{ exe: "chrome.exe", name: "Chrome" }]);
    expect(base.overlay.font_px).toBe(26);
  });
});



describe("shortcut recorder", () => {
  const press = (code: string, key: string, mods: Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>> = {}) =>
    recordShortcut({ code, key, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods });

  it("writes Tauri accelerators from the physical key, modifiers first", () => {
    expect(press("KeyA", "A", { ctrlKey: true, shiftKey: true })).toEqual({ kind: "shortcut", accelerator: "Ctrl+Shift+A" });
    expect(press("Digit7", "&", { altKey: true, shiftKey: true })).toEqual({ kind: "shortcut", accelerator: "Alt+Shift+7" });
    expect(press("F9", "F9")).toEqual({ kind: "shortcut", accelerator: "F9" });
  });

  it("waits for a full shortcut and reads Escape and Backspace on their own", () => {
    expect(press("KeyA", "a")).toEqual({ kind: "incomplete" });
    expect(press("ControlLeft", "Control", { ctrlKey: true })).toEqual({ kind: "incomplete" });
    expect(press("Escape", "Escape")).toEqual({ kind: "cancel" });
    expect(press("Backspace", "Backspace")).toEqual({ kind: "clear" });
  });
});
