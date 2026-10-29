// A bare page for exercising the screenshot backend by hand; the real window replaces it.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "../lib/tokens.css";

type Rect = { x: number; y: number; w: number; h: number };
type Block = { id: number; rect: Rect; source: string; translate: boolean };
type FrameInfo = { id: number; width: number; height: number; scale: number };
type ScreenEvent =
  | { type: "reading"; job: number }
  | { type: "blocks"; job: number; blocks: Block[]; ocr_ms: number }
  | { type: "translated"; job: number; id: number; text: string }
  | { type: "failed"; job: number; id: number; message: string }
  | { type: "done"; job: number; total_ms: number }
  | { type: "error"; job: number; message: string };

const element = <T extends HTMLElement>(id: string): T => {
  const found = document.getElementById(id);
  if (!found) throw new Error(`Missing #${id}`);
  return found as T;
};
const image = element<HTMLImageElement>("frame");
const layer = element<HTMLDivElement>("layer");
const box = element<HTMLDivElement>("box");
const status = element<HTMLDivElement>("status");

let frame: FrameInfo | null = null;
let job = 0;
let blocks = new Map<number, Block>();
let done = 0;
let released = 0;
let firstEnglish = 0;
let drag: { x: number; y: number } | null = null;
let selection = { left: 0, top: 0, right: 0, bottom: 0 };

function say(text: string): void {
  status.hidden = text === "";
  status.textContent = text;
  status.style.left = `${selection.left}px`;
  status.style.top = `${selection.bottom + 4}px`;
}

function clearJob(): void {
  layer.replaceChildren();
  blocks = new Map();
  done = 0;
  firstEnglish = 0;
  say("");
}

function reset(): void {
  frame = null;
  job = 0;
  image.removeAttribute("src");
  box.hidden = true;
  clearJob();
}

function close(): void {
  reset();
  void invoke("screen_close");
}

function draw(block: Block, text: string): void {
  if (!frame) return;
  const scale = frame.scale;
  const div = document.createElement("div");
  div.className = "block";
  div.style.left = `${block.rect.x / scale}px`;
  div.style.top = `${block.rect.y / scale}px`;
  div.style.width = `${block.rect.w / scale}px`;
  div.style.minHeight = `${block.rect.h / scale}px`;
  div.textContent = text;
  div.title = block.source;
  layer.append(div);
}

function onEvent(event: ScreenEvent): void {
  // Job ids grow, and a job's first events can arrive before `screen_select` answers.
  if (event.job < job) return;
  if (event.job > job) {
    job = event.job;
    clearJob();
  }
  const total = [...blocks.values()].filter((block) => block.translate).length;
  switch (event.type) {
    case "reading":
      say("Reading text…");
      break;
    case "blocks":
      blocks = new Map(event.blocks.map((block) => [block.id, block]));
      say(`Translating 0 of ${event.blocks.filter((block) => block.translate).length} (OCR ${event.ocr_ms} ms)`);
      break;
    case "translated":
    case "failed": {
      done += 1;
      if (firstEnglish === 0) firstEnglish = performance.now() - released;
      const block = blocks.get(event.id);
      if (block) draw(block, event.type === "translated" ? event.text : `⚠ ${event.message}`);
      say(`Translating ${done} of ${total}`);
      break;
    }
    case "done":
      say(`Done in ${event.total_ms} ms; first English ${Math.round(firstEnglish)} ms after mouse-up`);
      break;
    case "error":
      say(event.message);
      break;
  }
}

void listen<FrameInfo>("screen://frame", ({ payload }) => {
  reset();
  frame = payload;
  image.style.width = `${payload.width / payload.scale}px`;
  image.style.height = `${payload.height / payload.scale}px`;
  image.src = `http://ltshot.localhost/${payload.id}`;
});
void listen<ScreenEvent>("screen://event", ({ payload }) => onEvent(payload));

window.addEventListener("pointerdown", (event) => {
  if (event.button === 2) return close();
  if (event.button !== 0 || !frame) return;
  drag = { x: event.clientX, y: event.clientY };
  box.hidden = true;
});

window.addEventListener("pointermove", (event) => {
  if (!drag) return;
  selection = {
    left: Math.min(drag.x, event.clientX),
    top: Math.min(drag.y, event.clientY),
    right: Math.max(drag.x, event.clientX),
    bottom: Math.max(drag.y, event.clientY),
  };
  box.hidden = false;
  box.style.left = `${selection.left}px`;
  box.style.top = `${selection.top}px`;
  box.style.width = `${selection.right - selection.left}px`;
  box.style.height = `${selection.bottom - selection.top}px`;
});

window.addEventListener("pointerup", () => {
  const started = drag;
  drag = null;
  if (!started || !frame || box.hidden) return;
  if (selection.right - selection.left < 8 || selection.bottom - selection.top < 8) return;
  released = performance.now();
  const scale = frame.scale;
  const x = Math.floor(selection.left * scale);
  const y = Math.floor(selection.top * scale);
  const rect = { x, y, w: Math.ceil(selection.right * scale) - x, h: Math.ceil(selection.bottom * scale) - y };
  invoke<number>("screen_select", { frame: frame.id, rect }).then(
    (id) => {
      if (id > job) {
        job = id;
        clearJob();
      }
    },
    (error: unknown) => say(String(error)),
  );
});

window.addEventListener("contextmenu", (event) => event.preventDefault());
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") close();
});
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") reset();
});
