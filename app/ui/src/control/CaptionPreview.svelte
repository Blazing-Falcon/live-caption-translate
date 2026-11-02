<script lang="ts">
  import type { CaptionBlock } from "../lib/captions";
  import type { Config } from "../lib/settings";
  import CaptionPanel from "../overlay/CaptionPanel.svelte";
  import FlowText from "../overlay/FlowText.svelte";
  import SubtitleText from "../overlay/SubtitleText.svelte";

  let { overlay, scene }: { overlay: Config["overlay"]; scene: "dark" | "bright" | "game" } = $props();

  const block = (key: number, source: string, english: string): CaptionBlock => ({
    key,
    ids: [key],
    sealed: true,
    source,
    live: "",
    english,
    state: "final",
    reason: null,
    preview: null,
  });

  const SAMPLE: readonly CaptionBlock[] = [
    block(1, "你昨天晚上去哪儿了？", "Where were you last night?"),
    block(2, "我等了你整整一个小时。", "I waited a whole hour for you."),
    {
      key: 3,
      ids: [3],
      sealed: false,
      source: "",
      live: "公司临时开会，手机",
      english: "",
      state: "pending",
      reason: null,
      preview: "We had a last-minute meeting",
    },
  ];
</script>

<div class="screen" data-scene={scene} aria-label="Caption preview" role="img">
  <div
    class="overlay-root"
    data-style={overlay.style}
    data-card={overlay.card}
    data-font={overlay.font}
    data-edge={overlay.panel_edge}
    style:--font-px={overlay.font_px}
    style:--ov-bar-alpha={overlay.background}
    style:--cap-en={overlay.english_color}
    style:--cap-zh={overlay.chinese_color}
  >
    {#if overlay.style === "panel"}
      <CaptionPanel blocks={SAMPLE} showSource={overlay.show_source} panelLines={overlay.panel_lines} reducedMotion />
    {:else if overlay.style === "subtitles"}
      <SubtitleText blocks={SAMPLE} showSource={overlay.show_source} />
    {:else}
      <FlowText blocks={SAMPLE} showSource={overlay.show_source} flowLines={overlay.flow_lines} reducedMotion />
    {/if}
  </div>
</div>

<style>
  .screen {
    position: relative;
    height: 340px;
    overflow: hidden;
    border: var(--stroke-w) solid var(--stroke);
    border-radius: var(--r-ctl);
    background: var(--scene-dark);
    pointer-events: none;
  }

  .screen[data-scene="bright"] {
    background: var(--scene-bright);
  }

  .screen[data-scene="game"] {
    background: var(--scene-game);
  }

  .overlay-root {
    position: absolute;
    inset: 12px;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    color: var(--ov-text);
  }

  .overlay-root[data-style="panel"] {
    justify-content: stretch;
    width: min(400px, 60%);
  }

  .overlay-root[data-style="panel"][data-edge="right"] {
    left: auto;
  }

  .overlay-root[data-style="panel"][data-edge="left"] {
    right: auto;
  }

  .overlay-root[data-style="panel"] > :global(.panel) {
    flex: 1;
    min-height: 0;
  }
</style>
