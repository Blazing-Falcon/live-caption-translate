/** Caption color presets and the legibility rule for the night card. */
export interface ColorPreset {
  value: string;
  name: string;
}

export const ENGLISH_PRESETS: readonly ColorPreset[] = [
  { value: "#ffffff", name: "White" },
  { value: "#ffd84d", name: "Pencil yellow" },
  { value: "#fff1c9", name: "Cream" },
  { value: "#a9d8ff", name: "Sky" },
  { value: "#aef0cb", name: "Mint" },
  { value: "#ffbfd0", name: "Pink" },
];

export const CHINESE_PRESETS: readonly ColorPreset[] = [
  { value: "#c9c9c4", name: "Soft grey" },
  { value: "#ffffff", name: "White" },
  { value: "#ffe27a", name: "Yellow" },
  { value: "#b9dcff", name: "Sky" },
  { value: "#bdf2d4", name: "Mint" },
  { value: "#ffcad8", name: "Pink" },
];

/** The night card at full opacity; a see-through card over bright video only lowers contrast. */
export const NIGHT_CARD = "#1c1c1c";
/** WCAG AA for body text. */
export const MIN_CONTRAST = 4.5;

function channel(hex: string, offset: number): number {
  const value = parseInt(hex.slice(offset, offset + 2), 16) / 255;
  return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

function luminance(hex: string): number {
  return 0.2126 * channel(hex, 1) + 0.7152 * channel(hex, 3) + 0.0722 * channel(hex, 5);
}

export function contrastRatio(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (light + 0.05) / (dark + 0.05);
}

/** The warning for a caption color on the night card, or null when it reads well. */
export function nightCardWarning(color: string): string | null {
  const ratio = contrastRatio(color, NIGHT_CARD);
  if (ratio >= MIN_CONTRAST) return null;
  return `Hard to read on the dark card (${ratio.toFixed(1)}:1). Pick a lighter color.`;
}
