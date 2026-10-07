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
  it("round-trips through JSON", () => {
    expect(JSON.parse(JSON.stringify(DEFAULT_CONFIG))).toEqual(DEFAULT_CONFIG);
    expect(parseConfig(JSON.parse(JSON.stringify(DEFAULT_CONFIG)))).toEqual(DEFAULT_CONFIG);
  });

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

  it("mirrors all 13 tables", () => {
    const tables = Object.keys(DEFAULT_CONFIG).filter((key) => key !== "config_version");
    expect(tables).toHaveLength(13);
  });
});

describe("parseConfig", () => {
  it("rejects payloads that are not a full config", () => {
    expect(() => parseConfig(null)).toThrow(/unreadable/);
    expect(() => parseConfig({ config_version: 1 })).toThrow(/\[capture\]/);
    const broken = cloneConfig(DEFAULT_CONFIG) as unknown as Record<string, unknown>;
    delete broken.models;
    expect(() => parseConfig(broken)).toThrow(/\[models\]/);
  });

  it("keeps unknown keys", () => {
    const withExtra = { ...cloneConfig(DEFAULT_CONFIG), future_key: { a: 1 } };
    expect(parseConfig(withExtra)).toHaveProperty("future_key");
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

describe("patch helpers", () => {
  it("builds narrow nested patches", () => {
    expect(captureModePatch("apps")).toEqual({ capture: { mode: "apps" } });
    expect(overlayPatch({ background: 0.4 })).toEqual({ overlay: { background: 0.4 } });
    expect(hotkeyPatch("pause", "  Ctrl+Alt+P ")).toEqual({ hotkeys: { pause: "Ctrl+Alt+P" } });
    expect(hotkeyPatch("show_hide", "")).toEqual({ hotkeys: { show_hide: "" } });
    expect(translateThreadsPatch(0)).toEqual({ translate: { threads: 0 } });
    expect(translateThreadsPatch(99)).toEqual({ translate: { threads: 16 } });
    expect(translateThreadsPatch(3.6)).toEqual({ translate: { threads: 4 } });
  });

  it("preserves unknown enabled languages when toggling ja/ko", () => {
    expect(translateOtherPatch(["yue", "ko"], "ja", true)).toEqual({ routing: { translate_other: ["yue", "ko", "ja"] } });
    expect(translateOtherPatch(["yue", "ko"], "ko", false)).toEqual({ routing: { translate_other: ["yue"] } });
    expect(translateOtherPatch(["ja"], "ja", true)).toEqual({ routing: { translate_other: ["ja"] } });
    expect(translateOtherPatch(["ja-JP"], "ja", false)).toEqual({ routing: { translate_other: [] } });
  });

  it("matches and toggles apps by executable name, case-insensitively", () => {
    const apps = [
      { exe: "chrome.exe", name: "Google Chrome" },
      { exe: "Discord.exe", name: "Discord" },
    ];
    expect(hasApp(apps, "CHROME.EXE")).toBe(true);
    expect(hasApp(apps, "vlc.exe")).toBe(false);
    expect(toggledApps(apps, { exe: "vlc.exe", name: "VLC" }, true)).toEqual([...apps, { exe: "vlc.exe", name: "VLC" }]);
    expect(toggledApps(apps, { exe: "DISCORD.EXE", name: "Discord" }, true)).toEqual([
      apps[0],
      { exe: "DISCORD.EXE", name: "Discord" },
    ]);
    expect(toggledApps(apps, { exe: "discord.exe", name: "Discord" }, false)).toEqual([apps[0]]);
    expect(captureAppsPatch([])).toEqual({ capture: { apps: [] } });
  });
});

describe("language helpers", () => {
  it.each([
    ["ja", "ja"],
    ["<|ja|>", "ja"],
    ["ja-JP", "ja"],
    ["KO_kr", "ko"],
    ["jpn", "ja"],
    ["zho", "zh"],
    ["  ", null],
    [null, null],
  ])("normalizes %j to %j", (input, expected) => {
    expect(normalizeLang(input)).toBe(expected);
  });

  it("labels languages and falls back to a neutral label", () => {
    expect(languageLabel("ja")).toBe("JAPANESE");
    expect(languageLabel("<|ko|>")).toBe("KOREAN");
    expect(languageLabel("zh")).toBe("CHINESE");
    expect(languageLabel("yue")).toBe("CANTONESE");
    expect(languageLabel("en")).toBe("ENGLISH");
    expect(languageLabel("de")).toBe("DE");
    expect(languageLabel(null)).toBe("OTHER LANGUAGE");
  });

  it("checks enabled languages through normalization", () => {
    expect(isOtherLangTranslated(["ja"], "<|ja|>")).toBe(true);
    expect(isOtherLangTranslated(["ja"], "ko")).toBe(false);
    expect(isOtherLangTranslated(["ja"], null)).toBe(false);
  });

  it("formats accelerators the way the spec prints them", () => {
    expect(formatAccelerator("Ctrl+Shift+L")).toBe("Ctrl Shift L");
    expect(formatAccelerator("")).toBe("");
    expect(formatAccelerator(" Ctrl + P ")).toBe("Ctrl P");
  });
});
