import { describe, expect, it } from "vitest";
import { CHINESE_PRESETS, ENGLISH_PRESETS, contrastRatio, nightCardWarning } from "../src/lib/colors";

describe("caption colors", () => {
  it("every preset reads well on the night card", () => {
    for (const preset of [...ENGLISH_PRESETS, ...CHINESE_PRESETS]) expect(nightCardWarning(preset.value)).toBeNull();
  });

  it("warns below 4.5:1 and names the ratio", () => {
    expect(contrastRatio("#ffffff", "#000000")).toBeCloseTo(21, 5);
    expect(nightCardWarning("#3a3a8a")).toBe("Hard to read on the dark card (1.7:1). Pick a lighter color.");
    expect(nightCardWarning("#848484")).toBeNull();
    expect(nightCardWarning("#838383")).not.toBeNull();
  });
});
