// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { emit } from "@tauri-apps/api/event";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it } from "vitest";
import { isHiddenLive, visibleLines } from "../src/lib/captions";
import { createSession, EVENTS, type Session } from "../src/lib/events";
import type { CaptionLine as Line } from "../src/lib/types";
import CaptionLine from "../src/overlay/CaptionLine.svelte";
import CaptionPanel from "../src/overlay/CaptionPanel.svelte";
import OverlayApp from "../src/overlay/OverlayApp.svelte";
import SubtitleBar from "../src/overlay/SubtitleBar.svelte";
import { FakeBackend } from "./backend";
import { cleanup, q, render, settle } from "./dom";
import { FakeClock, asrFinal, delta, draft, final, partial, skipped } from "./helpers";

const line = (patch: Partial<Line> & Pick<Line, "id" | "state">): Line => ({
  source: "这次华为发布的呢",
  english: "",
  draft: "",
  drafts: [],
  shown: "",
  heldOnce: false,
  committed: false,
  lang: null,
  reason: null,
  updatedAt: 0,
  ...patch,
});

const read = (relative: string): string => readFileSync(new URL(relative, import.meta.url), "utf8");

afterEach(() => {
  cleanup();
  clearMocks();
});

describe("CaptionLine, live and draft states", () => {
  it("live without a draft shows the Chinese so far and nothing for English (no dots)", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "live" }) });
    expect(q(root, ".source").textContent).toBe("这次华为发布的呢");
    expect(root.querySelector(".english")).toBeNull();
    expect(root.querySelector(".dots")).toBeNull();
    expect(root.querySelector(".label")).toBeNull();
    expect(root.textContent).not.toContain("Translating");
  });

  it("live with a draft shows the displayed draft text in the draft style", () => {
    const root = render(CaptionLine, {
      line: line({ id: 1, state: "live", draft: "This time, Huawei released the whole", shown: "This time, Huawei released" }),
    });
    const english = q(root, ".english");
    expect(english.textContent).toBe("This time, Huawei released");
    expect(english.classList.contains("draft")).toBe(true);
    expect(root.querySelector(".caret")).toBeNull();
    expect(q(root, ".source").textContent).toBe("这次华为发布的呢");
  });

  it("live with live_source off and nothing to show is not rendered at all", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "live" }), liveSource: false });
    expect(root.querySelector("article")).toBeNull();
    expect(root.textContent).toBe("");
  });

  it("live with live_source off keeps the draft but hides the Chinese", () => {
    const root = render(CaptionLine, {
      line: line({ id: 1, state: "live", draft: "This time", shown: "This time" }),
      liveSource: false,
    });
    expect(root.querySelector(".source")).toBeNull();
    expect(q(root, ".english").textContent).toBe("This time");
    expect(q(root, ".english").classList.contains("draft")).toBe(true);
  });

  it("live_source does not hide the Chinese of committed lines", () => {
    const root = render(CaptionLine, {
      line: line({ id: 1, state: "pending", committed: true }),
      liveSource: false,
    });
    expect(q(root, ".source").textContent).toBe("这次华为发布的呢");
    expect(q(root, ".dots").textContent).toBe("···");
  });

  it("pending with a draft shows the whole draft in the draft style and no dots", () => {
    const root = render(CaptionLine, {
      line: line({ id: 1, state: "pending", committed: true, draft: "This time Huawei announced", shown: "This time Huawei announced" }),
      showSource: false,
    });
    expect(q(root, ".source").textContent).toBe("这次华为发布的呢");
    expect(q(root, ".english").textContent).toBe("This time Huawei announced");
    expect(q(root, ".english").classList.contains("draft")).toBe(true);
    expect(root.querySelector(".dots")).toBeNull();
    expect(root.querySelector(".caret")).toBeNull();
  });

  it("pending without a draft keeps the v1 dots", () => {
    const root = render(CaptionLine, { line: line({ id: 1, state: "pending", committed: true }) });
    expect(q(root, ".dots").textContent).toBe("···");
    expect(root.querySelector(".english.draft")).toBeNull();
  });

  it("streaming and final are not draft-styled", () => {
    const streaming = render(CaptionLine, { line: line({ id: 1, state: "streaming", english: "Thi" }) });
    expect(q(streaming, ".english").classList.contains("draft")).toBe(false);
    expect(streaming.querySelector(".caret")).not.toBeNull();
    const done = render(CaptionLine, {
      line: line({ id: 2, state: "final", english: "This time.", draft: "This time", shown: "This time" }),
    });
    expect(q(done, ".english").textContent).toBe("This time.");
    expect(q(done, ".english").classList.contains("draft")).toBe(false);
  });

  it("skipped with a draft shows the dimmed draft and no label", () => {
    const root = render(CaptionLine, {
      line: line({ id: 1, state: "skipped", committed: true, draft: "This time", shown: "This time" }),
    });
    expect(q(root, ".english").textContent).toBe("This time");
    expect(q(root, ".english").classList.contains("draft")).toBe(true);
    expect(root.querySelector(".label")).toBeNull();
    expect(q(root, "article").classList.contains("skipped")).toBe(true);
  });

  it("failed with a draft shows the dimmed draft and the v1 failure label under it", () => {
    const root = render(CaptionLine, {
      line: line({ id: 1, state: "failed", committed: true, reason: "Translator is restarting", draft: "This time", shown: "This time" }),
    });
    const children = [...q(root, "article").children].map((el) => [...el.classList].filter((name) => !name.startsWith("svelte-")).join(" "));
    expect(children).toEqual(["source", "english draft", "label warning"]);
    expect(q(root, ".label").textContent).toBe("Translation failed · Translator is restarting");
    expect(q(root, "article").classList.contains("dimmed")).toBe(true);
  });

  it("skipped or failed without a draft are exactly v1", () => {
    const s = render(CaptionLine, { line: line({ id: 1, state: "skipped", committed: true }) });
    expect(q(s, ".label").textContent).toBe("Skipped to catch up");
    expect(s.querySelector(".english")).toBeNull();
    const f = render(CaptionLine, { line: line({ id: 2, state: "failed", committed: true, reason: "x" }) });
    expect(f.querySelector(".english")).toBeNull();
    expect(q(f, "article").classList.contains("dimmed")).toBe(false);
  });
});

describe("draft style", () => {
  const css = read("../src/overlay/CaptionLine.svelte");
  const rule = (selector: string): string => {
    const start = css.indexOf(`${selector} {`);
    expect(start, selector).toBeGreaterThan(-1);
    return css.slice(start, css.indexOf("}", start));
  };

  it("defines the draft color token as #b4b7bc and uses only that token for draft text", () => {
    const tokens = read("../src/lib/tokens.css");
    expect(tokens).toMatch(/--ov-draft:\s*#b4b7bc;/);
    expect(tokens).toMatch(/--ov-chip-text:\s*#b4b7bc;/);
    expect(rule(".english.draft")).toContain("color: var(--ov-draft);");
    expect(rule(".english.draft").split(";")).toHaveLength(2);
  });

  it("adds no italics, blur, pulse, shimmer, caret, underline or icon for drafts", () => {
    const draftRule = rule(".english.draft");
    for (const forbidden of ["italic", "blur", "animation", "text-decoration", "underline", "filter", "opacity", "::after", "::before"]) {
      expect(draftRule).not.toContain(forbidden);
    }
    expect(css).not.toContain("italic");
    expect(css).not.toContain("blur(");
    expect(css).not.toContain("shimmer");
  });

  it("draft text has the same font, size, weight and line height as the final English", () => {
    const english = rule(".english");
    expect(english).toContain("font-size: calc(var(--font-px, 26) * 1px * var(--line-scale))");
    expect(english).toContain("font-weight: 500");
    expect(english).toContain("line-height: 1.3");
    expect(rule(".english.draft")).not.toMatch(/font|line-height/);
  });

  it("changes the color with a 120 ms linear transition that reduced motion removes", () => {
    expect(rule(".english")).toContain("transition: color var(--recolor-ms) linear");
    const tokens = read("../src/lib/tokens.css");
    expect(tokens).toMatch(/--recolor-ms:\s*120ms;/);
    const reduced = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(tokens)?.[1] ?? "";
    expect(reduced).toContain("transition: none !important");
    expect(reduced).toContain("animation: none !important");
  });
});

describe("hidden live lines", () => {
  it("only a live line without text to show is hidden, and only when live_source is off", () => {
    expect(isHiddenLive(line({ id: 1, state: "live" }), false)).toBe(true);
    expect(isHiddenLive(line({ id: 1, state: "live" }), true)).toBe(false);
    expect(isHiddenLive(line({ id: 1, state: "live", shown: "Hi" }), false)).toBe(false);
    expect(isHiddenLive(line({ id: 1, state: "pending" }), false)).toBe(false);
  });

  it("visibleLines skips hidden live lines so they take no slot", () => {
    const lines = [
      line({ id: 1, state: "final", english: "One" }),
      line({ id: 2, state: "final", english: "Two" }),
      line({ id: 3, state: "live" }),
    ];
    expect(visibleLines(lines, "bar", 5, false).map((l) => l.id)).toEqual([1, 2]);
    expect(visibleLines(lines, "bar", 5, true).map((l) => l.id)).toEqual([2, 3]);
    expect(visibleLines(lines, "bar", 5).map((l) => l.id)).toEqual([2, 3]);
  });

  it("the bar shows no bar at all for a lone hidden live line and keeps both older lines otherwise", () => {
    const lone = render(SubtitleBar, { lines: [line({ id: 3, state: "live" })], liveSource: false });
    expect(lone.querySelector("section")).toBeNull();
    const lines = [
      line({ id: 1, state: "final", english: "One" }),
      line({ id: 2, state: "final", english: "Two" }),
      line({ id: 3, state: "live" }),
    ];
    const root = render(SubtitleBar, { lines, liveSource: false });
    expect([...root.querySelectorAll("article")].map((a) => a.getAttribute("data-id"))).toEqual(["1", "2"]);
    const shown = render(SubtitleBar, { lines, liveSource: true });
    expect([...shown.querySelectorAll("article")].map((a) => a.getAttribute("data-id"))).toEqual(["2", "3"]);
  });

  it("the panel keeps its slots for visible lines", () => {
    const lines = Array.from({ length: 5 }, (_, index) => line({ id: index + 1, state: "final", english: `L${index + 1}` }));
    lines.push(line({ id: 6, state: "live" }));
    const root = render(CaptionPanel, { lines, panelLines: 4, liveSource: false });
    expect([...root.querySelectorAll("article")].map((a) => a.getAttribute("data-id"))).toEqual(["2", "3", "4", "5"]);
    const withLive = render(CaptionPanel, { lines, panelLines: 4, liveSource: true });
    expect([...withLive.querySelectorAll("article")].map((a) => a.getAttribute("data-id"))).toEqual(["3", "4", "5", "6"]);
  });

  it("marks the live line as the newest line", () => {
    const lines = [line({ id: 1, state: "final", english: "One" }), line({ id: 2, state: "live" })];
    const root = render(SubtitleBar, { lines });
    const articles = [...root.querySelectorAll("article")];
    expect(articles[0]?.classList.contains("older")).toBe(true);
    expect(articles[1]?.classList.contains("older")).toBe(false);
    expect(articles[1]?.getAttribute("data-state")).toBe("live");
  });
});

describe("OverlayApp with live lines and drafts", () => {
  const sessions: Session[] = [];

  async function open(configure?: (backend: FakeBackend) => void) {
    const backend = new FakeBackend();
    configure?.(backend);
    backend.install();
    const session = createSession({ kind: "overlay", clock: new FakeClock(), reducedMotion: () => false });
    sessions.push(session);
    const root = render(OverlayApp, { session });
    await session.ready;
    await settle();
    return { backend, session, root };
  }

  const send = async (...events: unknown[]) => {
    for (const event of events) await emit(EVENTS.pipeline, event);
    await settle();
  };

  afterEach(() => sessions.splice(0).forEach((session) => session.dispose()));

  it("walks a clause from live Chinese to a dimmed draft to the final in the same elements", async () => {
    const { root } = await open();
    await send(partial(1, "这次华为"));
    expect(q(root, '[data-state="live"] .source').textContent).toBe("这次华为");
    expect(root.querySelector(".english")).toBeNull();
    await send(draft(1, "This time Huawei released the whole"), partial(1, "这次华为发布的呢是整个"));
    const article = q(root, "article");
    const english = q(root, ".english");
    expect(english.textContent).toBe("This time Huawei released");
    expect(english.classList.contains("draft")).toBe(true);
    await send(asrFinal(1, "这次华为发布的呢是整个mate90家族啊，", "chinese", "zh", "commit"));
    expect(q(root, "article")).toBe(article);
    expect(q(root, ".english")).toBe(english);
    expect(english.textContent).toBe("This time Huawei released the whole");
    expect(english.classList.contains("draft")).toBe(true);
    expect(article.getAttribute("data-state")).toBe("pending");
    await send(final(1, "This time, Huawei released the whole Mate 90 family."));
    expect(root.querySelectorAll("article")).toHaveLength(1);
    expect(q(root, "article")).toBe(article);
    expect(q(root, ".english")).toBe(english);
    expect(english.textContent).toBe("This time, Huawei released the whole Mate 90 family.");
    expect(english.classList.contains("draft")).toBe(false);
    expect(article.getAttribute("data-state")).toBe("final");
    expect(root.querySelector(".caret")).toBeNull();
  });

  it("keeps the draft visible while the final streams in (no caret) and after a skip", async () => {
    const { root } = await open();
    await send(partial(1, "你好"), draft(1, "Hello there my friend"), asrFinal(1, "你好，我的朋友。"), delta(1, "Hello"));
    expect(q(root, ".english").textContent).toBe("Hello there my friend");
    expect(root.querySelector(".caret")).toBeNull();
    await send(skipped(1));
    expect(q(root, ".english").textContent).toBe("Hello there my friend");
    expect(root.querySelector(".label")).toBeNull();
  });

  it("hides the live Chinese when overlay.live_source is false and reserves no space for a line without a draft", async () => {
    const { root } = await open((b) => (b.config.overlay.live_source = false));
    await send(partial(1, "这次华为"));
    expect(root.querySelector("article")).toBeNull();
    expect(root.querySelector('[data-testid="subtitle-bar"]')).toBeNull();
    await send(draft(1, "This time Huawei"));
    expect(root.querySelector(".source")).toBeNull();
    expect(q(root, ".english").textContent).toBe("This time");
    await send(asrFinal(1, "这次华为。"));
    expect(q(root, ".source").textContent).toBe("这次华为。");
  });

  it("shows the Chinese of a live line by default", async () => {
    const { root } = await open();
    await send(partial(1, "这次华为"));
    expect(q(root, '[data-state="live"] .source').textContent).toBe("这次华为");
  });

  it("drops a live line that turns out to be music or silence", async () => {
    const { root } = await open();
    await send(partial(1, "啦啦啦"), { type: "dropped", id: 1, reason: "music" });
    expect(root.querySelector("article")).toBeNull();
  });
});
