import type { RectF, ScreenBlock } from "./types";

export const ENGLISH_FONT = '"Segoe UI", "Microsoft JhengHei", sans-serif';
export const ENGLISH_FONT_WEIGHT = 600;
export const ENGLISH_LINE_SPACING = 1.330078125;

const SMALL_SOURCE_HEIGHT = 12.0;
const LARGE_SOURCE_HEIGHT = 16.0;
const SMALL_MULTIPLIER = 1.18;
const LARGE_MULTIPLIER = 1.06;
const SMALL_MIN_FONT_SIZE = 14.5;
const LARGE_MIN_FONT_SIZE = 13.0;
const SMALL_LATIN_TO_CJK_BOOST = 1.08;
const LATIN_TO_CJK_BOOST_CAP = 1.25;

const OVERLAY_PADDING = 6;
const BUBBLE_EXPAND = 2;
const BUBBLE_MIN_WIDTH = 30;
const BUBBLE_HORIZONTAL_PADDING = 6;
const BUBBLE_VERTICAL_PADDING = 6;
const SINGLE_LINE_ABSOLUTE_MIN_FONT_SIZE = 9.5;
const SINGLE_LINE_READABLE_MIN_FONT_SIZE = 10.0;
const SINGLE_LINE_EMERGENCY_MIN_FONT_SIZE = 7.0;
const WRAPPED_ABSOLUTE_MIN_FONT_SIZE = 11.0;
const GROUPED_EMERGENCY_MIN_FONT_SIZE = 7.0;
const MAX_LINE_COUNT = 2147483647;

const clamp = (value: number, minimum: number, maximum: number): number => Math.min(Math.max(value, minimum), maximum);

function blend(sourceHeight: number, atSmall: number, atLarge: number, unitScale = 1): number {
  const t = clamp((sourceHeight - SMALL_SOURCE_HEIGHT * unitScale) / ((LARGE_SOURCE_HEIGHT - SMALL_SOURCE_HEIGHT) * unitScale), 0, 1);
  return atSmall + (atLarge - atSmall) * t;
}

export const minFontSize = (sourceHeight: number, unitScale = 1): number => blend(sourceHeight, SMALL_MIN_FONT_SIZE * unitScale, LARGE_MIN_FONT_SIZE * unitScale, unitScale);

export function sourceFontSize(sourceHeight: number, latinSourceToCjkTarget = false, unitScale = 1): number {
  let size = sourceHeight * blend(sourceHeight, SMALL_MULTIPLIER, LARGE_MULTIPLIER, unitScale);
  if (latinSourceToCjkTarget) {
    size = Math.min(size * blend(sourceHeight, SMALL_LATIN_TO_CJK_BOOST, 1.0, unitScale), size + LATIN_TO_CJK_BOOST_CAP * unitScale);
  }
  return Math.max(SMALL_MIN_FONT_SIZE * unitScale, size);
}

export interface SingleLineInput {
  originalFontSize: number;
  originalBorderWidth: number;
  textWidth: number;
  horizontalPadding: number;
  rightExpansionWidth: number;
  maxExpandedWidth: number;
  readableMinFontSize: number;
  absoluteMinFontSize: number;
}

export interface SingleLineResult {
  fontSize: number;
  borderWidth: number;
  preferRightExpansion: boolean;
}

export function singleLineLayout(input: SingleLineInput): SingleLineResult {
  const idealBorderWidth = Math.max(input.originalBorderWidth, input.textWidth + input.horizontalPadding);
  if (idealBorderWidth <= input.rightExpansionWidth) {
    return { fontSize: input.originalFontSize, borderWidth: idealBorderWidth, preferRightExpansion: true };
  }
  const scaleFont = (targetWidth: number): number => input.textWidth <= 0 ? input.originalFontSize : input.originalFontSize * targetWidth / input.textWidth;
  const originalInnerWidth = Math.max(1, input.originalBorderWidth - input.horizontalPadding);
  const scaledToOriginalWidth = Math.min(input.originalFontSize, scaleFont(originalInnerWidth));
  if (scaledToOriginalWidth >= input.readableMinFontSize) {
    return { fontSize: scaledToOriginalWidth, borderWidth: input.originalBorderWidth, preferRightExpansion: false };
  }
  const readableWidth = input.originalFontSize <= 0 ? input.horizontalPadding :
    input.textWidth * input.readableMinFontSize / input.originalFontSize + input.horizontalPadding;
  const expandedBorderWidth = Math.min(input.maxExpandedWidth, Math.max(input.originalBorderWidth, readableWidth));
  const scaledAfterExpansion = scaleFont(Math.max(1, expandedBorderWidth - input.horizontalPadding));
  return {
    fontSize: Math.max(input.absoluteMinFontSize, Math.min(input.readableMinFontSize, scaledAfterExpansion)),
    borderWidth: expandedBorderWidth,
    preferRightExpansion: false,
  };
}

export function wrappedBubbleHeight(sourceBorderHeight: number, preferredHeight: number, wrappedTextHeight: number, capHeight: number | null): number {
  const wanted = Math.max(preferredHeight, wrappedTextHeight);
  return capHeight === null ? wanted : Math.max(wrappedTextHeight, Math.min(wanted, Math.max(capHeight, sourceBorderHeight)));
}

export interface TextSize { width: number; height: number }
export interface Measurer {
  measure(text: string, fontSize: number, maxTextWidth?: number): TextSize;
}

export interface LayoutContext {
  width: number;
  height: number;
  selection: RectF;
  scale?: number;
}

export interface TextLayout {
  bubble: RectF;
  text: RectF;
  fontSize: number;
  sourceFontSize: number;
  wrap: boolean;
  wrappedLineCount: number;
  writtenHeight: number;
}

const bounds = (block: ScreenBlock): RectF => block.paint?.bounds ?? { x: block.rect.x, y: block.rect.y, width: block.rect.w, height: block.rect.h };
const sourceLines = (block: ScreenBlock): RectF[] => block.paint?.source_lines ?? block.lines.map(r => ({ x: r.x, y: r.y, width: r.w, height: r.h }));

export function sourceFontReferenceHeight(block: ScreenBlock, fallbackHeight: number): number {
  const glyph = block.paint?.render_glyph_height;
  if (glyph !== null && glyph !== undefined && glyph > 0) return glyph;
  const heights = sourceLines(block).map(r => r.height).sort((a, b) => a - b);
  return heights.length ? heights[Math.floor(heights.length / 2)]! : fallbackHeight;
}

export function sourceFontSizingHeight(block: ScreenBlock, fallbackHeight: number): number {
  const glyph = block.paint?.font_glyph_height;
  return glyph !== null && glyph !== undefined && glyph > 0 ? glyph : sourceFontReferenceHeight(block, fallbackHeight);
}

export function estimateWrappedLineCount(text: string, size: number, width: number, measurer: Measurer, unitScale = 1): number {
  const singleLine = measurer.measure(text, size);
  if (singleLine.width <= width) return 1;
  const wrapped = measurer.measure(text, size, width);
  const lineHeight = Math.max(unitScale, measurer.measure("Ag", size).height);
  return Math.max(1, Math.ceil((wrapped.height - 0.1 * unitScale) / lineHeight));
}

export function findLargestGroupedFontSize(text: string, preferred: number, minimum: number, width: number, maxLines: number, maxHeight: number, measurer: Measurer, unitScale = 1): number {
  for (let size = preferred; size >= minimum; size -= 0.5 * unitScale) {
    const wrapped = measurer.measure(text, size, width);
    if (estimateWrappedLineCount(text, size, width, measurer, unitScale) <= maxLines && wrapped.height + BUBBLE_VERTICAL_PADDING * unitScale <= maxHeight) return size;
  }
  for (let size = minimum - 0.5 * unitScale; size >= GROUPED_EMERGENCY_MIN_FONT_SIZE * unitScale; size -= 0.5 * unitScale) {
    const wrapped = measurer.measure(text, size, width);
    if (estimateWrappedLineCount(text, size, width, measurer, unitScale) <= maxLines && wrapped.height + BUBBLE_VERTICAL_PADDING * unitScale <= maxHeight) return size;
  }
  return GROUPED_EMERGENCY_MIN_FONT_SIZE * unitScale;
}

export function bottomAvailableHeight(current: ScreenBlock, blocks: ScreenBlock[], context: LayoutContext): number {
  const box = bounds(current);
  const expand = BUBBLE_EXPAND * (context.scale ?? 1), padding = OVERLAY_PADDING * (context.scale ?? 1);
  const left = box.x - expand;
  const right = left + box.width + expand * 2;
  let bottomLimit = Math.min(context.height - padding, context.selection.y + context.selection.height);
  for (const other of blocks) {
    const next = bounds(other);
    if (other === current || next.y <= box.y) continue;
    const otherLeft = next.x - expand;
    const otherRight = otherLeft + next.width + expand * 2;
    if (otherLeft < right && otherRight > left) bottomLimit = Math.min(bottomLimit, next.y - padding);
  }
  return Math.max(0, bottomLimit - (box.y - expand));
}

export function hasLowerOverlappingBlock(current: ScreenBlock, blocks: ScreenBlock[]): boolean {
  const box = bounds(current);
  return blocks.some(other => {
    const next = bounds(other);
    return other !== current && next.y > box.y && next.x < box.x + box.width && next.x + next.width > box.x;
  });
}

export function rightExpansionWidth(current: ScreenBlock, blocks: ScreenBlock[], context: LayoutContext): number {
  const box = bounds(current);
  const expand = BUBBLE_EXPAND * (context.scale ?? 1), padding = OVERLAY_PADDING * (context.scale ?? 1);
  const left = box.x - expand;
  const top = box.y - expand;
  const bottom = top + box.height + expand * 2;
  let rightLimit = Math.min(context.width - padding, context.selection.x + context.selection.width);
  for (const other of blocks) {
    const next = bounds(other);
    if (other === current || next.x <= box.x) continue;
    const otherTop = next.y - expand;
    const otherBottom = otherTop + next.height + expand * 2;
    if (otherTop < bottom && otherBottom > top) rightLimit = Math.min(rightLimit, next.x - expand - padding);
  }
  return Math.max(0, rightLimit - left);
}

export function horizontalLayout(block: ScreenBlock, text: string, blocks: ScreenBlock[], context: LayoutContext, measurer: Measurer): TextLayout {
  const unitScale = context.scale ?? 1;
  const expand = BUBBLE_EXPAND * unitScale, padding = OVERLAY_PADDING * unitScale;
  const horizontalPadding = BUBBLE_HORIZONTAL_PADDING * unitScale, verticalPadding = BUBBLE_VERTICAL_PADDING * unitScale;
  const absoluteMinimum = SINGLE_LINE_ABSOLUTE_MIN_FONT_SIZE * unitScale;
  const box = bounds(block);
  const sizingHeight = sourceFontSizingHeight(block, box.height);
  const borderW = Math.max(box.width + expand * 2, BUBBLE_MIN_WIDTH * unitScale);
  const borderH = box.height + expand * 2;
  const lines = sourceLines(block);
  const singleLine = !/[\n\r]/.test(block.source) && sizingHeight <= 28 * unitScale;
  const grouped = lines.length > 1;
  const reflowGroup = block.intent === "group_reflow";
  const minimum = minFontSize(sizingHeight, unitScale);
  const preferred = sourceFontSize(sizingHeight, false, unitScale);
  let fontSize = preferred;
  const availableWidth = Math.max(BUBBLE_MIN_WIDTH * unitScale, context.width - padding * 2);
  let targetBorderW = borderW;
  let preferRightExpansion = false;
  let wrap = false;
  let maxWrapHeight: number | null = null;
  const measured = measurer.measure(text, fontSize);
  let innerW = Math.max(unitScale, borderW - horizontalPadding);
  if (grouped) {
    wrap = true;
    const maxLines = hasLowerOverlappingBlock(block, blocks) ? lines.length : lines.length + 1;
    if (reflowGroup) targetBorderW = borderW;
    else targetBorderW = Math.min(availableWidth, Math.max(borderW, Math.min(measured.width + horizontalPadding, rightExpansionWidth(block, blocks, context))));
    preferRightExpansion = targetBorderW > borderW;
    maxWrapHeight = bottomAvailableHeight(block, blocks, context);
    fontSize = findLargestGroupedFontSize(text, fontSize, absoluteMinimum, Math.max(unitScale, targetBorderW - horizontalPadding), maxLines, maxWrapHeight, measurer, unitScale);
  } else if (singleLine) {
    const result = singleLineLayout({ originalFontSize: fontSize, originalBorderWidth: borderW, textWidth: measured.width,
      horizontalPadding, rightExpansionWidth: rightExpansionWidth(block, blocks, context),
      maxExpandedWidth: availableWidth, readableMinFontSize: SINGLE_LINE_READABLE_MIN_FONT_SIZE * unitScale, absoluteMinFontSize: absoluteMinimum });
    fontSize = result.fontSize;
    targetBorderW = result.borderWidth;
    preferRightExpansion = result.preferRightExpansion;
    let finalMeasure = measurer.measure(text, fontSize);
    const width = Math.max(unitScale, targetBorderW - horizontalPadding);
    let overflows = finalMeasure.width > width;
    const emergency = Math.min(fontSize, SINGLE_LINE_EMERGENCY_MIN_FONT_SIZE * unitScale);
    for (let attempt = 0; attempt < 3 && overflows; attempt++) {
      const fitted = Math.max(emergency, fontSize * width / finalMeasure.width);
      if (fitted >= fontSize) break;
      fontSize = fitted;
      finalMeasure = measurer.measure(text, fontSize);
      overflows = finalMeasure.width > width;
    }
    if (overflows) {
      wrap = true;
      maxWrapHeight = bottomAvailableHeight(block, blocks, context);
      fontSize = findLargestGroupedFontSize(text, preferred, absoluteMinimum, width, MAX_LINE_COUNT, maxWrapHeight, measurer, unitScale);
    }
  } else if (measured.width > innerW) {
    const scaled = fontSize * innerW / measured.width;
    if (scaled >= minimum) {
      fontSize = scaled;
      let fitted = measurer.measure(text, fontSize);
      for (let attempt = 0; attempt < 3 && fitted.width > innerW; attempt++) {
        const next = Math.max(minimum, fontSize * innerW / fitted.width);
        if (next >= fontSize) break;
        fontSize = next;
        fitted = measurer.measure(text, fontSize);
      }
      if (fitted.width > innerW) wrap = true;
    } else {
      fontSize = Math.max(WRAPPED_ABSOLUTE_MIN_FONT_SIZE * unitScale, minimum);
      wrap = true;
    }
  }
  if (!wrap && /[\n\r]/.test(text)) wrap = true;
  let actualBorderH = Math.max(borderH, fontSize + verticalPadding);
  let writtenHeight = fontSize;
  if (wrap) {
    innerW = Math.max(unitScale, targetBorderW - horizontalPadding);
    writtenHeight = measurer.measure(text, fontSize, innerW).height;
    actualBorderH = wrappedBubbleHeight(borderH, actualBorderH, writtenHeight + verticalPadding, maxWrapHeight);
  }
  const expandedOffsetX = (targetBorderW - borderW) / 2;
  const left = clamp(box.x - expand - (preferRightExpansion ? 0 : expandedOffsetX), padding, Math.max(padding, context.width - targetBorderW - padding));
  const top = clamp(box.y - expand, padding, Math.max(padding, context.height - actualBorderH - padding));
  const height = measurer.measure(text, fontSize, wrap ? Math.max(unitScale, targetBorderW - horizontalPadding) : undefined).height;
  return {
    bubble: { x: left, y: top, width: targetBorderW, height: actualBorderH },
    text: { x: left + horizontalPadding / 2, y: top + (actualBorderH - height) / 2, width: Math.max(0, targetBorderW - horizontalPadding), height },
    fontSize, sourceFontSize: preferred, wrap, writtenHeight,
    wrappedLineCount: wrap ? estimateWrappedLineCount(text, fontSize, Math.max(unitScale, targetBorderW - horizontalPadding), measurer, unitScale) : 1,
  };
}
