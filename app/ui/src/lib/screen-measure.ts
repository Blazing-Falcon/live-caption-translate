// Measures English text in the browser with the same font and line spacing it is drawn with.
import { toCssPx, toFramePx } from "./screen";
import { ENGLISH_FONT, ENGLISH_FONT_WEIGHT, ENGLISH_LINE_SPACING, type Measurer, type TextSize } from "./screen-layout";

export class DomMeasurer implements Measurer {
  private readonly element: HTMLDivElement;
  // Results by text, size and width, since the layout searches ask for the same measurement
  // repeatedly.
  private readonly cache = new Map<string, TextSize>();

  constructor(private readonly scale: number) {
    this.element = document.createElement("div");
    Object.assign(this.element.style, {
      position: "fixed", left: "-100000px", top: "0", visibility: "hidden",
      pointerEvents: "none", margin: "0", border: "0", padding: "0",
      boxSizing: "content-box", fontFamily: ENGLISH_FONT, fontWeight: String(ENGLISH_FONT_WEIGHT),
      lineHeight: String(ENGLISH_LINE_SPACING), overflowWrap: "anywhere", wordBreak: "normal",
    });
    this.element.setAttribute("aria-hidden", "true");
    document.body.append(this.element);
  }

  measure(text: string, fontSize: number, maxTextWidth?: number): TextSize {
    const key = JSON.stringify([text, fontSize, maxTextWidth]);
    const known = this.cache.get(key);
    if (known) return known;
    this.element.textContent = text;
    this.element.style.fontSize = `${toCssPx(fontSize, this.scale)}px`;
    this.element.style.whiteSpace = maxTextWidth === undefined ? "pre" : "pre-wrap";
    this.element.style.width = maxTextWidth === undefined ? "max-content" : `${toCssPx(Math.max(0, maxTextWidth), this.scale)}px`;
    const box = this.element.getBoundingClientRect();
    const range = document.createRange();
    range.selectNodeContents(this.element);
    // A wrapping box is as wide as its limit, so the width of the text itself is read from a range
    // over it.
    const ink = range.getBoundingClientRect();
    const size = {
      width: toFramePx(maxTextWidth === undefined ? box.width : Math.min(box.width, ink.width), this.scale),
      height: toFramePx(box.height, this.scale),
    };
    this.cache.set(key, size);
    return size;
  }

  dispose(): void {
    this.element.remove();
    this.cache.clear();
  }
}
