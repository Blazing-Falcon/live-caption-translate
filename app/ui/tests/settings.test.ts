import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  DEFAULT_CONFIG,
  applyPatch,
  captureAppsPatch,
  captureModePatch,
  cloneConfig,
  formatAccelerator,
  hasApp,
  hotkeyPatch,
  isOtherLangTranslated,
  languageLabel,
  normalizeLang,
  overlayPatch,
  parseConfig,
  toggledApps,
  translateOtherPatch,
  translateThreadsPatch,
} from "../src/lib/settings";

function parseDefaultToml(): Record<string, unknown> {
  const doc = readFileSync(new URL("../../../docs/configuration.md", import.meta.url), "utf8");
  const block = /## Full Default File\s+```toml\r?\n([\s\S]*?)```/.exec(doc)?.[1];
  if (!block) throw new Error("default config block not found in docs/configuration.md");
  const root: Record<string, unknown> = {};
  let table: Record<string, unknown> = root;
  for (const raw of block.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    const header = /^\[([a-z_]+)\]/.exec(line);
    if (header?.[1]) {
      table = {};
      root[header[1]] = table;
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

});


describe("applyPatch", () => {
  it("merges nested tables, replaces arrays and removes keys set to null", () => {
    const base = cloneConfig(DEFAULT_CONFIG);
    base.overlay.bar_rect = { monitor: "A", x: 1, y: 2, w: 3, h: 4 };
    const next = applyPatch(base, {
      overlay: { font_px: 30, bar_rect: null },
      routing: { translate_other: ["ko"] },
    });
    expect(next.overlay.font_px).toBe(30);
    expect(next.overlay.style).toBe("bar");
    expect(next.overlay.bar_rect).toBeUndefined();
    expect(next.routing.translate_other).toEqual(["ko"]);
    expect(base.overlay.font_px).toBe(26);
  });
});


