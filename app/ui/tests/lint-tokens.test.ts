import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { lintSource, lintTree } from "../scripts/lint-tokens.mjs";

const messages = (source: string): string[] => lintSource(source).map((problem) => problem.message);

describe("token lint", () => {
  it("accepts var() tokens and non-color values", () => {
    const source = `<style>
      a { color: var(--c-text); background: transparent; border: 1px solid var(--c-border); fill: currentColor; }
      .x { outline: 2px solid var(--c-focus); box-shadow: none; }
    </style>
    <a href="#top" aria-label="x">link</a>
    <p>&#123; entity &#x41; and #hash text</p>
    <circle fill="var(--ov-move-bg)" />`;
    expect(messages(source)).toEqual([]);
  });

  it("rejects hex colors of every length", () => {
    expect(messages("<style>a{color:#fff}</style>")).toHaveLength(1);
    expect(messages("<style>a{color:#ffff}</style>")).toHaveLength(1);
    expect(messages("<style>a{background: #1B1C1E}</style>")).toHaveLength(1);
    expect(messages("<div style=\"color: #1b1c1eaa\"></div>")).toHaveLength(1);
  });

  it("rejects color functions but allows var()-based channels", () => {
    expect(messages("<style>a{color: rgb(1 2 3)}</style>")).not.toEqual([]);
    expect(messages("<style>a{color: rgba(1,2,3,.5)}</style>")).not.toEqual([]);
    expect(messages("<style>a{color: hsl(10 20% 30%)}</style>")).not.toEqual([]);
    expect(messages("<style>a{color: oklch(0.5 0.1 20)}</style>")).not.toEqual([]);
    expect(messages("<style>a{background: rgb(var(--ground) / 0.5)}</style>")).toEqual([]);
  });

  it("rejects named colors in color properties and svg attributes", () => {
    expect(messages("<style>a{color: white}</style>")).toEqual(['named color "white"']);
    expect(messages("<style>a{border: 1px solid red}</style>")).toEqual(['named color "red"']);
    expect(messages('<path stroke="black" />')).toEqual(['literal svg color "black"']);
    expect(messages("<style>.white-space{font-weight:500}</style>")).toEqual([]);
  });

  it("ignores colors inside html comments", () => {
    expect(messages("<!-- #fff -->")).toEqual([]);
  });

  it("finds no literal colors in the shipped components", () => {
    const failures = lintTree(fileURLToPath(new URL("../src", import.meta.url)));
    expect(failures).toEqual([]);
  });
});
