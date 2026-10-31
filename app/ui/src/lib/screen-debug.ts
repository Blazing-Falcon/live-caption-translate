import type { Rect, RectF, ScreenBlock } from "./types";

export const GROUP_OUTSET = 3;
export const lineBoxes = (blocks: ScreenBlock[]): Rect[] => blocks.flatMap(block => block.lines);
export const groupBoxes = (blocks: ScreenBlock[]): RectF[] => blocks.map(block => {
  const rect = block.paint.bounds;
  return { x: rect.x - GROUP_OUTSET, y: rect.y - GROUP_OUTSET, width: rect.width + GROUP_OUTSET * 2, height: rect.height + GROUP_OUTSET * 2 };
});
