// @vitest-environment jsdom
import { clearMocks } from "@tauri-apps/api/mocks";
import { flushSync, mount, unmount } from "svelte";
import { get } from "svelte/store";
import { afterEach, describe, expect, it } from "vitest";
import { createSession, type Session } from "../src/lib/events";
import {
  formatVariants,
  glossaryPatch,
  splitVariants,
  type GlossaryEntry,
} from "../src/lib/settings";
import { FakeBackend } from "./backend";
import GlossaryHarness from "./GlossaryHarness.svelte";
import { FakeClock } from "./helpers";

const ENTRY: GlossaryEntry = {
  zh: "卓总",
  en: "President Zhuo",
  note: "name, male, her boss",
  variants: ["周总", "朱总", "翟总"],
  confirmed: true,
};

describe("also heard as", () => {
  it("splits on a comma, a full-width comma and an enumeration mark, trimming and dropping repeats", () => {
    expect(splitVariants("周总, 朱总，翟总、周总 ,,  ")).toEqual(["周总", "朱总", "翟总"]);
    expect(splitVariants("")).toEqual([]);
    expect(splitVariants("   ")).toEqual([]);
  });

  it("joins with a comma and a space, and splits back to the same list", () => {
    expect(formatVariants(["周总", "朱总"])).toBe("周总, 朱总");
    expect(splitVariants(formatVariants(ENTRY.variants))).toEqual(ENTRY.variants);
  });
});

describe("glossary page", () => {
  const sessions: Session[] = [];
  let host: HTMLElement | null = null;
  let page: Record<string, unknown> | null = null;

  afterEach(() => {
    if (page) unmount(page as Parameters<typeof unmount>[0]);
    page = null;
    host?.remove();
    host = null;
    sessions.splice(0).forEach((session) => session.dispose());
    clearMocks();
  });

  async function open(entries: GlossaryEntry[]): Promise<{ session: Session; backend: FakeBackend }> {
    const backend = new FakeBackend();
    backend.config.translate.glossary = structuredClone(entries);
    backend.install();
    const session = createSession({ kind: "control", clock: new FakeClock(), reducedMotion: () => false });
    sessions.push(session);
    await session.ready;
    host = document.createElement("div");
    document.body.appendChild(host);
    page = mount(GlossaryHarness, { target: host, props: { session } }) as Record<string, unknown>;
    flushSync();
    return { session, backend };
  }

  function element<T extends HTMLElement>(testId: string): T {
    const found = document.querySelector<T>(`[data-testid="${testId}"]`);
    if (!found) throw new Error(`no ${testId}`);
    return found;
  }

  function type(testId: string, value: string): void {
    const input = element<HTMLInputElement>(testId);
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
  }

  function change(testId: string, value: string): void {
    const input = element<HTMLInputElement>(testId);
    input.value = value;
    input.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
  }

  /** The handler awaits the save, so one macrotask turn lets it land. */
  const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

  it("normalises the list without saving when nothing really changed", async () => {
    const { backend } = await open([ENTRY]);

    expect(element<HTMLInputElement>("glossary-variants-0").value).toBe("周总, 朱总, 翟总");
    // Mixed separators, a blank and a repeat: the same list, written differently.
    change("glossary-variants-0", "周总，朱总、翟总, 翟总 ,, 朱总");
    await settle();

    expect(backend.last("set_config")).toBeUndefined();
    expect(element<HTMLInputElement>("glossary-variants-0").value).toBe("周总, 朱总, 翟总");
  });

  it("saves when a variant is really new", async () => {
    const { backend } = await open([ENTRY]);

    change("glossary-variants-0", "周总，朱总、翟总、小绿");
    await settle();

    expect(backend.config.translate.glossary[0]?.variants).toEqual(["周总", "朱总", "翟总", "小绿"]);
    expect(element<HTMLInputElement>("glossary-variants-0").value).toBe("周总, 朱总, 翟总, 小绿");
  });

  it("saves a typed name as a confirmed entry with every field", async () => {
    const { session, backend } = await open([]);

    expect(element("glossary-empty")).toBeTruthy();
    element("glossary-add").click();
    flushSync();
    type("glossary-new-zh", " 肖律 ");
    type("glossary-new-variants", "小绿，小龙");
    type("glossary-new-en", "Lawyer Xiao");
    type("glossary-new-note", "lawyer, female");
    element("glossary-save").click();
    await settle();

    const saved = [{ zh: "肖律", en: "Lawyer Xiao", note: "lawyer, female", variants: ["小绿", "小龙"], confirmed: true }];
    expect(backend.last("set_config")?.args).toEqual({ patch: { translate: { glossary: saved } } });
    expect(get(session.config)?.translate.glossary).toEqual(saved);
  });

  it("deletes a row by sending the list without it", async () => {
    const second = { ...ENTRY, zh: "肖律", en: "Lawyer Xiao", variants: ["小绿"] };
    const { session, backend } = await open([ENTRY, second]);

    element("glossary-delete-0").click();
    await settle();

    expect(backend.config.translate.glossary).toEqual([second]);
    expect(get(session.config)?.translate.glossary).toEqual([second]);
  });

  it("saves a text field without dropping the other fields of the entry", async () => {
    const { backend } = await open([ENTRY]);

    change("glossary-note-0", " her boss ");
    await settle();

    expect(backend.config.translate.glossary).toEqual([{ ...ENTRY, note: "her boss" }]);
  });
});
