<script lang="ts">
  import CaptionLine from "../../src/overlay/CaptionLine.svelte";
  import type { CaptionLine as Line } from "../../src/lib/types";

  const base = { english: "", lang: "zh", reason: null, updatedAt: 0 };
  const zh = "你别误会，我不是那个意思。";

  const cells: { title: string; line?: Line; status?: { kind: "no_device"; text: string }; dim?: boolean }[] = [
    { title: "PENDING · speech ended, waiting for translation", line: { ...base, id: 1, state: "pending", source: zh } },
    { title: "STREAMING · words arrive, amber caret at the end", line: { ...base, id: 2, state: "streaming", source: zh, english: "Don't get it wrong, that's" } },
    { title: "FINAL", line: { ...base, id: 3, state: "final", source: zh, english: "Don't get it wrong, that's not what I mean." } },
    { title: "ENGLISH SPEECH · shown as heard, no source line", line: { ...base, id: 4, state: "english", source: "OK, can you guys hear me now?", lang: "en" } },
    { title: "OTHER LANGUAGE · untranslated unless enabled", line: { ...base, id: 5, state: "other", source: "みんな、ありがとう！", lang: "ja" } },
    { title: "SKIPPED · caught up to live speech", line: { ...base, id: 6, state: "skipped", source: "然后我们再去下一个地图看看。" } },
    { title: "FAILED · source stays, short reason", line: { ...base, id: 7, state: "failed", source: "这件事说来话长。", reason: "Translator is restarting" } },
    { title: "STATUS · replaces captions when nothing can be heard", status: { kind: "no_device", text: "No audio device" } },
  ];
</script>

<main class="board">
  <h1>Caption line states</h1>
  <p>Each line is keyed by its utterance and updates in place. Only the newest line is full strength; older lines fade by age.</p>
  <div class="grid">
    {#each cells as cell (cell.title)}
      <div class="cell">
        <div class="title">{cell.title}</div>
        <div class="box">
          {#if cell.line}
            <CaptionLine line={cell.line} variant="panel" age={0} showSource />
          {:else if cell.status}
            <CaptionLine status={cell.status} />
          {/if}
        </div>
      </div>
    {/each}
  </div>
</main>

<style>
  .board {
    --font-px: 22;
    box-sizing: border-box;
    width: 1280px;
    min-height: 760px;
    padding: 48px 56px;
    background: rgb(28 29 32);
    color: var(--ov-text);
  }

  h1 {
    margin: 0 0 6px;
    font-size: 22px;
    font-weight: 600;
  }

  p {
    max-width: 760px;
    margin: 0 0 32px;
    font-size: 14px;
    line-height: 1.5;
    color: var(--ov-meta);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 20px 32px;
  }

  .title {
    margin-bottom: 10px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--ov-meta);
  }

  .box {
    padding: 14px 18px;
    background: rgb(var(--ov-ground-rgb));
    border-radius: var(--radius-card);
  }
</style>
