import { expect, it } from "vitest";
import { ENGLISH_FONT, ENGLISH_FONT_WEIGHT } from "../src/lib/screen-layout";

it("uses Segoe UI with JhengHei fallback for an English translation and interface", () => {
  expect(ENGLISH_FONT.replaceAll('"', "").toLowerCase()).toBe("segoe ui, microsoft jhenghei, sans-serif");
});

it("sets English translations in semibold", () => {
  expect(ENGLISH_FONT_WEIGHT).toBe(600);
});
