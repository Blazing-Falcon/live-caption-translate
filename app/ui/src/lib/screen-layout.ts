// English layout for screenshot blocks: font size, single-line fitting, wrapping and the room each
// block may use. Lengths are frame pixels scaled by the layout context.
import type { RectF, ScreenBlock, ScreenTilt } from "./types";

export const ENGLISH_FONT = '"Segoe UI", "Microsoft JhengHei", sans-serif';
export const ENGLISH_FONT_WEIGHT = 600;
// Line height factor shared by the measurer and the drawn text, so a measured height matches what
// is painted.
export const ENGLISH_LINE_SPACING = 1.330078125;

// Source text at or below this height counts as small and at or above the large height as large;
// sizes between blend linearly.
const SMALL_SOURCE_HEIGHT = 12.0;
const LARGE_SOURCE_HEIGHT = 16.0;
// English font size as a multiple of the source height. Small text is enlarged more.
const SMALL_MULTIPLIER = 1.18;
const LARGE_MULTIPLIER = 1.06;
// Smallest preferred English font size for small and large source text.
const SMALL_MIN_FONT_SIZE = 14.5;
const LARGE_MIN_FONT_SIZE = 13.0;
// A Latin source drawn as CJK is nudged larger, since CJK glyphs lose legibility first. The nudge
// is capped in absolute terms.
const SMALL_LATIN_TO_CJK_BOOST = 1.08;
const LATIN_TO_CJK_BOOST_CAP = 1.25;

// Margin kept between a block and the edge of the frame or selection.
const OVERLAY_PADDING = 6;
// How far a bubble reaches past its source box on every side.
const BUBBLE_EXPAND = 2;
// Narrowest bubble, so a very short label still holds a word.
const BUBBLE_MIN_WIDTH = 30;
// Total padding inside a bubble across its width, half on each side.
const BUBBLE_HORIZONTAL_PADDING = 6;
// Total padding inside a bubble down its height, half on each side.
const BUBBLE_VERTICAL_PADDING = 6;
// Smallest font a single line is shrunk to before it is wrapped instead.
const SINGLE_LINE_ABSOLUTE_MIN_FONT_SIZE = 9.5;
// Font size down to which a single line is shrunk before its box is widened to keep it readable.
const SINGLE_LINE_READABLE_MIN_FONT_SIZE = 10.0;
// Last-resort size for a single line that still overflows after widening.
const SINGLE_LINE_EMERGENCY_MIN_FONT_SIZE = 7.0;
// Smallest font for text that is too long for its box and is wrapped rather than shrunk further.
const WRAPPED_ABSOLUTE_MIN_FONT_SIZE = 11.0;
// Smallest font a wrapped group is ever drawn at.
const GROUPED_EMERGENCY_MIN_FONT_SIZE = 7.0;
// No limit on the number of lines.
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

// Keeps the size when the box can grow to the right. Otherwise it shrinks the text to the original
// width while it stays readable, and then widens the box.
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

// A wrapped bubble grows to hold its text, but not past the room below unless the source box was
// already taller.
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
  tilt?: ScreenTilt;
  positioned?: boolean;
  textAlign?: "left" | "center";
  content?: string;
}

export interface AcrossInColumnsRoom { left: number; right: number; below: number }
export interface AcrossInColumnsFit { fontSize: number; left: number; top: number; width: number; height: number }
// If every word is this short or shorter, the text may shrink into the emergency size range. A
// longer word widens the box instead.
export const ORDINARY_WORD_LENGTH = 12;
// Font size step of the fitting searches.
const SIZE_STEP = 0.5;
// How much taller than its own lines a group off a tilted card may be set.
const TILTED_HEIGHT_ALLOWANCE = 1.15;
// Characters the text is split into words on.
const TEXT_WHITESPACE = /[\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+/u;

// Free space to the left, right and below a block before it meets another block.
export function acrossInColumnsRoom(source: RectF, others: RectF[], limit: RectF): AcrossInColumnsRoom {
  const sourceRight = source.x + source.width, sourceBottom = source.y + source.height;
  let left = Math.max(0, source.x - limit.x);
  let right = Math.max(0, (limit.x + limit.width) - sourceRight);
  let below = Math.max(0, (limit.y + limit.height) - sourceBottom);
  for (const other of others) {
    if (other.width < 0 || (other.x === source.x && other.y === source.y && other.width === source.width && other.height === source.height)) continue;
    const otherRight = other.x + other.width, otherBottom = other.y + other.height;
    const sharesHeight = other.y < sourceBottom && otherBottom > source.y;
    const sharesWidth = other.x < sourceRight && otherRight > source.x;
    if (sharesHeight && otherRight <= source.x) left = Math.min(left, (source.x - otherRight) / 2);
    else if (sharesHeight && other.x >= sourceRight) right = Math.min(right, (other.x - sourceRight) / 2);
    else if (sharesWidth && other.y >= sourceBottom) below = Math.min(below, other.y - sourceBottom);
    else if (sharesHeight && sharesWidth) {
      if (other.x + otherRight < source.x + sourceRight) left = 0;
      else right = 0;
      if (other.y + otherBottom > source.y + sourceBottom) below = 0;
    }
  }
  return { left, right, below };
}

// Finds the largest font whose wrapped text fits the room. The searches go from keeping the source
// footprint, to using all the room, to allowing a long word to take two or more lines, to the
// emergency size.
export function acrossInColumnsFit(text: string, sourceWidth: number, sourceHeight: number, preferredFontSize: number,
  readableFontSize: number, emergencyFontSize: number, room: AcrossInColumnsRoom, measurer: Measurer, keepSize = false, unitScale = 1): AcrossInColumnsFit {
  sourceWidth = Math.max(unitScale, sourceWidth);
  sourceHeight = Math.max(unitScale, sourceHeight);
  emergencyFontSize = Math.max(unitScale, emergencyFontSize);
  readableFontSize = Math.max(emergencyFontSize, readableFontSize);
  preferredFontSize = Math.max(readableFontSize, preferredFontSize);
  const widest = sourceWidth + room.left + room.right;
  const comfortable = keepSize ? widest : Math.min(widest, sourceWidth + preferredFontSize * 2);
  const tallest = sourceHeight + room.below;
  const groupHeight = keepSize ? tallest : sourceHeight;
  const words = text.split(TEXT_WHITESPACE).filter(Boolean);
  const step = SIZE_STEP * unitScale;
  type Fit = { size: number; width: number; height: number };
  function longestWord(size: number): number {
    let longest = 0;
    for (const word of words) longest = Math.max(longest, measurer.measure(word, size).width);
    return Math.ceil(longest / unitScale) * unitScale + unitScale;
  }
  function narrowest(size: number, low: number, high: number, highHeight: number, maxHeight: number): Fit {
    const atLow = measurer.measure(text, size, low);
    if (atLow.height <= maxHeight) return { size, width: low, height: atLow.height };
    while (high - low > unitScale) {
      const middle = (low + high) / 2;
      const measured = measurer.measure(text, size, middle);
      if (measured.height <= maxHeight) { high = middle; highHeight = measured.height; }
      else low = middle;
    }
    return { size, width: high, height: highHeight };
  }
  function search(from: number, to: number, maxWidth: number, maxHeight: number, maxPieces = 1): Fit | null {
    for (let size = from; size >= to - 0.001 * unitScale; size -= step) {
      let low = Math.max(sourceWidth, longestWord(size));
      if (low > maxWidth) {
        if (low > maxWidth * maxPieces) continue;
        low = Math.max(sourceWidth, maxWidth);
      }
      const atWidest = measurer.measure(text, size, maxWidth);
      if (atWidest.height > maxHeight) continue;
      return narrowest(size, low, maxWidth, atWidest.height, maxHeight);
    }
    return null;
  }
  let fit = search(preferredFontSize, readableFontSize, comfortable, groupHeight)
    ?? search(preferredFontSize, readableFontSize, widest, groupHeight)
    ?? search(preferredFontSize, readableFontSize, widest, tallest)
    ?? (words.every(word => word.length <= ORDINARY_WORD_LENGTH) ? search(readableFontSize - step, emergencyFontSize, widest, tallest) : null)
    ?? search(preferredFontSize, readableFontSize, widest, tallest, 2)
    ?? search(preferredFontSize, readableFontSize, widest, tallest, MAX_LINE_COUNT)
    ?? search(readableFontSize - step, emergencyFontSize, widest, tallest, MAX_LINE_COUNT);
  if (!fit) {
    const width = Math.max(sourceWidth, Math.min(widest, longestWord(emergencyFontSize)));
    fit = { size: emergencyFontSize, width, height: measurer.measure(text, emergencyFontSize, width).height };
  }
  let left = (sourceWidth - fit.width) / 2;
  left = Math.max(left, -room.left);
  left = Math.min(left, sourceWidth + room.right - fit.width);
  left = Math.min(0, left);
  return { fontSize: fit.size, left, top: fit.height <= sourceHeight ? (sourceHeight - fit.height) / 2 : 0, width: fit.width, height: fit.height };
}

const bounds = (block: ScreenBlock): RectF => block.paint.bounds;
const sourceLines = (block: ScreenBlock): RectF[] => block.paint.source_lines;

export function sourceFontReferenceHeight(block: ScreenBlock, fallbackHeight: number): number {
  const glyph = block.paint.render_glyph_height;
  if (glyph !== null && glyph !== undefined && glyph > 0) return glyph;
  const heights = sourceLines(block).map(r => r.height).sort((a, b) => a - b);
  return heights.length ? heights[Math.floor(heights.length / 2)]! : fallbackHeight;
}

export function sourceFontSizingHeight(block: ScreenBlock, fallbackHeight: number): number {
  const glyph = block.paint.font_glyph_height;
  return glyph !== null && glyph !== undefined && glyph > 0 ? glyph : sourceFontReferenceHeight(block, fallbackHeight);
}

export function estimateWrappedLineCount(text: string, size: number, width: number, measurer: Measurer, unitScale = 1): number {
  const singleLine = measurer.measure(text, size);
  if (singleLine.width <= width) return 1;
  const wrapped = measurer.measure(text, size, width);
  const lineHeight = Math.max(unitScale, measurer.measure("Ag", size).height);
  return Math.max(1, Math.ceil((wrapped.height - 0.1 * unitScale) / lineHeight));
}

// Largest size from the preferred one down that wraps within the line and height limits. Below the
// minimum it continues to the emergency size.
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

// Height available below a block until the next block that overlaps it in width, or the end of the
// selection.
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

// Width available to the right of a block until the next block that shares its rows, or the end of
// the selection.
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

function tiltedLayout(block: ScreenBlock, text: string, context: LayoutContext, measurer: Measurer): TextLayout {
  const tilt = block.tilt!;
  const box = tilt.box;
  const scale = context.scale ?? 1;
  const expand = BUBBLE_EXPAND * scale;
  const width = Math.max(box.width + expand * 2, BUBBLE_MIN_WIDTH * scale);
  const innerWidth = Math.max(scale, width - BUBBLE_HORIZONTAL_PADDING * scale);
  const preferred = sourceFontSize(sourceFontSizingHeight(block, box.height), false, scale);
  const fontSize = findLargestGroupedFontSize(text, preferred, SINGLE_LINE_ABSOLUTE_MIN_FONT_SIZE * scale,
    innerWidth, MAX_LINE_COUNT, box.height * TILTED_HEIGHT_ALLOWANCE + expand * 2, measurer, scale);
  const writtenHeight = measurer.measure(text, fontSize, innerWidth).height;
  const height = Math.max(box.height + expand * 2, writtenHeight + BUBBLE_VERTICAL_PADDING * scale);
  const bubble = { x: box.x - expand, y: box.y + box.height / 2 - height / 2, width, height };
  return { bubble, text: { x: bubble.x + BUBBLE_HORIZONTAL_PADDING * scale / 2, y: bubble.y + (height - writtenHeight) / 2, width: innerWidth, height: writtenHeight },
    fontSize, sourceFontSize: preferred, wrap: true, writtenHeight, tilt, positioned: true, textAlign: "left",
    wrappedLineCount: estimateWrappedLineCount(text, fontSize, innerWidth, measurer, scale) };
}

function columnLayout(block: ScreenBlock, text: string, neighbours: ScreenBlock[], context: LayoutContext, measurer: Measurer): TextLayout {
  const tilt = block.tilt?.column ? block.tilt : undefined;
  const box = tilt?.box ?? bounds(block);
  const scale = context.scale ?? 1;
  const expand = BUBBLE_EXPAND * scale;
  const padX = BUBBLE_HORIZONTAL_PADDING * scale / 2;
  const padY = BUBBLE_VERTICAL_PADDING * scale / 2;
  const glyph = Math.min(sourceFontReferenceHeight(block, box.width), box.width);
  let room: AcrossInColumnsRoom = { left: 0, right: 0, below: 0 };
  if (!tilt) {
    const padding = OVERLAY_PADDING * scale;
    const left = Math.max(context.selection.x, padding), top = Math.max(context.selection.y, padding);
    const right = Math.min(context.selection.x + context.selection.width, context.width - padding);
    const bottom = Math.min(context.selection.y + context.selection.height, context.height - padding);
    if (right >= left && bottom >= top) room = acrossInColumnsRoom(box, neighbours.filter(other => other !== block).map(bounds),
      { x: left, y: top, width: right - left, height: bottom - top });
    room = { left: Math.max(0, room.left - padX), right: Math.max(0, room.right - padX), below: Math.max(0, room.below - padY) };
  }
  const written = text.replace(/^[\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+|[\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+$/gu, "");
  const fit = acrossInColumnsFit(written, box.width, box.height, glyph * 0.92, SINGLE_LINE_ABSOLUTE_MIN_FONT_SIZE * scale,
    GROUPED_EMERGENCY_MIN_FONT_SIZE * scale, room, measurer, false, scale);
  const textBox = { x: box.x + fit.left, y: box.y + fit.top, width: fit.width, height: fit.height };
  const left = Math.min(box.x - expand, textBox.x - padX), top = Math.min(box.y - expand, textBox.y - padY);
  const right = Math.max(box.x + box.width + expand, textBox.x + textBox.width + padX);
  const bottom = Math.max(box.y + box.height + expand, textBox.y + textBox.height + padY);
  const bubble = { x: left, y: top, width: right - left, height: bottom - top };
  return { bubble, text: textBox, fontSize: fit.fontSize, sourceFontSize: glyph * 0.92, wrap: true,
    writtenHeight: tilt ? bubble.height : fit.height, tilt, positioned: true, textAlign: "center", content: written,
    wrappedLineCount: estimateWrappedLineCount(written, fit.fontSize, fit.width, measurer, scale) };
}

// Chooses the layout by the kind of block: columns, a tilted card, or ordinary horizontal text.
export function blockLayout(block: ScreenBlock, text: string, blocks: ScreenBlock[], context: LayoutContext, measurer: Measurer, neighbours = blocks): TextLayout {
  if (block.intent === "across_in_columns") return columnLayout(block, text, neighbours, context, measurer);
  if (block.tilt && !block.tilt.column) return tiltedLayout(block, text, context, measurer);
  return horizontalLayout(block, text, blocks, context, measurer);
}

export function tiltTransform(tilt: ScreenTilt): string {
  return tilt.sheared ? `skewY(${tilt.degrees}deg)` : `rotate(${tilt.degrees}deg)`;
}
