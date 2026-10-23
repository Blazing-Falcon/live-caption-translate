<script lang="ts">
  import { get } from "svelte/store";
  import { useSession } from "../../lib/context";
  import { reconcileChecked } from "../../lib/reconcile";
  import {
    formatVariants,
    glossaryPatch,
    splitVariants,
    type Config,
    type GlossaryEntry,
  } from "../../lib/settings";

  let { config }: { config: Config } = $props();

  const session = useSession();
  const { config: liveConfig, busy } = session;

  const KEY = "translate.glossary";
  type Field = "zh" | "en" | "note";

  const entries = $derived(config.translate.glossary);

  /** The stored list. A patch replaces the array whole, so every edit starts from this. */
  const stored = (): GlossaryEntry[] =>
    get(liveConfig)?.translate.glossary ?? config.translate.glossary;

  // Saves run one after another: two edits that both started from the old list would undo
  // one another, because each patch carries the whole list.
  let saves: Promise<boolean> = Promise.resolve(true);

  function saveList(edit: (list: GlossaryEntry[]) => GlossaryEntry[]): Promise<boolean> {
    const run = saves.then(async () => {
      const saved = await session.save(KEY, glossaryPatch(edit(stored())));
      return saved !== null;
    });
    saves = run;
    return run;
  }

  function patchRow(index: number, change: (entry: GlossaryEntry) => GlossaryEntry): Promise<boolean> {
    return saveList((list) => list.map((entry, at) => (at === index ? change(entry) : entry)));
  }

  /** Enter leaves the field, which is when the change is saved. */
  function leaveOnEnter(event: KeyboardEvent): void {
    if (event.key === "Enter") (event.currentTarget as HTMLInputElement).blur();
  }

  async function commitText(index: number, field: Field, event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const value = input.value.trim();
    if (value !== stored()[index]?.[field]) {
      await patchRow(index, (entry) => ({ ...entry, [field]: value }));
    }
    // Show what was stored: the trimmed value, or the old one after a failed save.
    input.value = stored()[index]?.[field] ?? "";
  }

  async function commitVariants(index: number, event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const variants = splitVariants(input.value);
    if (formatVariants(variants) !== formatVariants(stored()[index]?.variants ?? [])) {
      await patchRow(index, (entry) => ({ ...entry, variants }));
    }
    // Show the stored list, which drops blanks and repeats and uses one comma style.
    input.value = formatVariants(stored()[index]?.variants ?? variants);
  }

  async function toggleConfirmed(index: number, event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const checked = input.checked;
    await patchRow(index, (entry) => ({ ...entry, confirmed: checked }));
    reconcileChecked(input, stored()[index]?.confirmed ?? checked);
  }

  async function remove(index: number): Promise<void> {
    if (get(busy).has(KEY)) return;
    await saveList((list) => list.filter((_, at) => at !== index));
  }

  let adding = $state(false);
  let saving = $state(false);
  let draft = $state({ zh: "", variants: "", en: "", note: "" });

  const canAdd = $derived(draft.zh.trim() !== "" && draft.en.trim() !== "");

  function beginAdd(): void {
    draft = { zh: "", variants: "", en: "", note: "" };
    adding = true;
  }

  async function addEntry(): Promise<void> {
    if (!canAdd || saving) return;
    const entry: GlossaryEntry = {
      zh: draft.zh.trim(),
      en: draft.en.trim(),
      note: draft.note.trim(),
      variants: splitVariants(draft.variants),
      // A name the user typed is meant to be used; a suggestion arrives unconfirmed instead.
      confirmed: true,
    };
    saving = true;
    if (await saveList((list) => [...list, entry])) adding = false;
    saving = false;
  }
</script>

<section class="page" aria-labelledby="glossary-title">
  <h2 id="glossary-title" class="section-title">Names and terms</h2>

  {#if entries.length === 0 && !adding}
    <div class="card empty" data-testid="glossary-empty">
      <p class="empty-title">No names yet</p>
      <p class="help">
        Names the recognizer hears several ways go here, so one person keeps one English name.
      </p>
      <div>
        <button type="button" class="btn primary" data-testid="glossary-add" onclick={beginAdd}>
          Add a name
        </button>
      </div>
    </div>
  {:else}
    <div class="table-wrap">
      <table class="table" data-testid="glossary-table">
        <thead>
          <tr>
            <th scope="col">Chinese</th>
            <th scope="col">Also heard as</th>
            <th scope="col">English</th>
            <th scope="col">Note</th>
            <th scope="col">Confirmed</th>
            <th scope="col"><span class="sr-only">Actions</span></th>
          </tr>
        </thead>
        <tbody>
          {#each entries as entry, index (index)}
            <tr data-testid={`glossary-row-${index}`}>
              <td>
                <input
                  class="input cell cjk"
                  type="text"
                  lang="zh"
                  spellcheck="false"
                  autocomplete="off"
                  placeholder="卓总"
                  value={entry.zh}
                  aria-label={`Chinese, row ${index + 1}`}
                  data-testid={`glossary-zh-${index}`}
                  onchange={(event) => commitText(index, "zh", event)}
                  onkeydown={leaveOnEnter}
                />
              </td>
              <td>
                <input
                  class="input cell cjk"
                  type="text"
                  lang="zh"
                  spellcheck="false"
                  autocomplete="off"
                  placeholder="周总, 朱总"
                  value={formatVariants(entry.variants)}
                  aria-label={`Also heard as, row ${index + 1}`}
                  data-testid={`glossary-variants-${index}`}
                  onchange={(event) => commitVariants(index, event)}
                  onkeydown={leaveOnEnter}
                />
              </td>
              <td>
                <input
                  class="input cell"
                  type="text"
                  spellcheck="false"
                  autocomplete="off"
                  placeholder="President Zhuo"
                  value={entry.en}
                  aria-label={`English, row ${index + 1}`}
                  data-testid={`glossary-en-${index}`}
                  onchange={(event) => commitText(index, "en", event)}
                  onkeydown={leaveOnEnter}
                />
              </td>
              <td>
                <input
                  class="input cell"
                  type="text"
                  autocomplete="off"
                  placeholder="name, male, her boss"
                  value={entry.note}
                  aria-label={`Note, row ${index + 1}`}
                  data-testid={`glossary-note-${index}`}
                  onchange={(event) => commitText(index, "note", event)}
                  onkeydown={leaveOnEnter}
                />
              </td>
              <td class="center">
                <input
                  type="checkbox"
                  checked={entry.confirmed}
                  aria-label={`Use ${entry.en.trim() || entry.zh.trim() || `row ${index + 1}`}`}
                  data-testid={`glossary-confirmed-${index}`}
                  onchange={(event) => toggleConfirmed(index, event)}
                />
              </td>
              <td class="center">
                <button
                  type="button"
                  class="btn compact"
                  aria-label={`Delete ${entry.zh.trim() || entry.en.trim() || `row ${index + 1}`}`}
                  aria-disabled={$busy.has(KEY)}
                  data-testid={`glossary-delete-${index}`}
                  onclick={() => remove(index)}
                >
                  Delete
                </button>
              </td>
            </tr>
          {/each}

          {#if adding}
            <tr class="new-row" data-testid="glossary-new-row">
              <td>
                <input
                  class="input cell cjk"
                  type="text"
                  lang="zh"
                  spellcheck="false"
                  autocomplete="off"
                  placeholder="卓总"
                  aria-label="New name, Chinese"
                  data-testid="glossary-new-zh"
                  bind:value={draft.zh}
                  onkeydown={leaveOnEnter}
                />
              </td>
              <td>
                <input
                  class="input cell cjk"
                  type="text"
                  lang="zh"
                  spellcheck="false"
                  autocomplete="off"
                  placeholder="周总, 朱总"
                  aria-label="New name, also heard as"
                  data-testid="glossary-new-variants"
                  bind:value={draft.variants}
                  onkeydown={leaveOnEnter}
                />
              </td>
              <td>
                <input
                  class="input cell"
                  type="text"
                  spellcheck="false"
                  autocomplete="off"
                  placeholder="President Zhuo"
                  aria-label="New name, English"
                  data-testid="glossary-new-en"
                  bind:value={draft.en}
                  onkeydown={leaveOnEnter}
                />
              </td>
              <td>
                <input
                  class="input cell"
                  type="text"
                  autocomplete="off"
                  placeholder="name, male, her boss"
                  aria-label="New name, note"
                  data-testid="glossary-new-note"
                  bind:value={draft.note}
                  onkeydown={leaveOnEnter}
                />
              </td>
              <td colspan="2">
                <div class="new-actions">
                  <button
                    type="button"
                    class="btn compact primary"
                    disabled={!canAdd}
                    aria-disabled={saving}
                    data-testid="glossary-save"
                    onclick={addEntry}
                  >
                    Add
                  </button>
                  <button type="button" class="btn compact" onclick={() => (adding = false)}>Cancel</button>
                </div>
              </td>
            </tr>
          {/if}
        </tbody>
      </table>
    </div>

    <p class="help small">
      Every spelling under "Also heard as" is replaced with the Chinese spelling before translation.
      Only a row with both spellings and the box ticked is used.
    </p>

    <div>
      <button
        type="button"
        class="btn"
        disabled={adding}
        aria-disabled={$busy.has(KEY)}
        data-testid="glossary-add"
        onclick={beginAdd}
      >
        Add a name
      </button>
    </div>
  {/if}

  <div class="stack">
    <h3 class="section-title">Suggestions</h3>
    <div class="card" data-testid="glossary-suggestions">
      <p class="help">
        Nothing yet. Suggestions appear here once a session has heard the same name more than once.
      </p>
    </div>
  </div>
</section>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 520px;
  }

  .empty-title {
    margin: 0;
    font-weight: 500;
  }

  .table-wrap {
    overflow-x: auto;
    background: var(--c-surface);
    border: 1px solid var(--c-border);
    border-radius: var(--radius-card);
  }

  .table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
  }

  .table th {
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--c-border);
    font-size: var(--text-help);
    font-weight: 600;
    text-align: left;
    color: var(--c-text-secondary);
  }

  .table th:nth-child(1) {
    width: 15%;
  }

  .table th:nth-child(2) {
    width: 23%;
  }

  .table th:nth-child(3) {
    width: 19%;
  }

  .table th:nth-child(4) {
    width: 21%;
  }

  .table th:nth-child(5) {
    width: 9%;
  }

  .table th:nth-child(6) {
    width: 13%;
  }

  .table td {
    padding: var(--space-1) var(--space-2);
    border-bottom: 1px solid var(--c-row-sep);
    vertical-align: middle;
  }

  .table tbody tr:last-child td {
    border-bottom: 0;
  }

  .cell {
    width: 100%;
    min-width: 0;
    height: 32px;
    font-size: var(--text-label);
  }

  .cjk {
    font-family: var(--font-cjk);
  }

  .center {
    text-align: center;
  }

  .btn.compact {
    min-width: 0;
    height: 28px;
    padding: 0 var(--space-2);
    font-size: var(--text-help);
  }

  .new-row td {
    background: var(--c-row-sep);
  }

  .new-actions {
    display: flex;
    gap: var(--space-2);
  }
</style>
