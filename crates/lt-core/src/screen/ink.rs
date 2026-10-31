//! Measures the height of the ink inside a line box, a truer text size than the box itself.

use super::{geom::RectF, line::Line, ImageView};

/// Boxes shorter than this have too few pixels to measure.
const MIN_BOX_HEIGHT: f64 = 6.0;
/// Only wide boxes (a line of text, not a label or a single glyph) are measured.
const MIN_BOX_ASPECT: f64 = 8.0;
/// Wide boxes are sampled every few pixels so the cost does not grow with width.
const MAX_ROW_SAMPLES: i32 = 512;
/// Colors are bucketed to 4 bits per channel so anti-aliasing noise votes for one background.
const QUANTIZATION_SHIFT: u32 = 4;
const QUANTIZATION_STEP: i32 = 16;
const QUANTIZATION_MIDPOINT: i32 = 8;
/// If no single color covers this share of the box there is no plain background to measure against.
const MIN_BACKGROUND_SHARE: f64 = 0.45;
/// A pixel counts as ink when any channel differs from the background by more than this.
const MAX_BACKGROUND_DELTA: i32 = 40;
/// Ink along the top or bottom edge means the box does not hold the text cleanly, so it is not
/// measured.
const MAX_BORDER_INK_SHARE: f64 = 0.1;
/// A row needs this many ink samples at its busiest, or the box holds specks rather than text.
const MIN_ROW_PEAK: i32 = 3;
/// A row counts as part of the text when its ink exceeds this share of the busiest row.
const MIN_ROW_PEAK_SHARE: f64 = 0.1;
/// Ink shorter than this is not a text height.
const MIN_INK_HEIGHT: i32 = 4;
/// Ink that fills the box is background noise, not text, so the box height is kept.
const MAX_INK_HEIGHT_SHARE: f64 = 0.95;

pub fn annotate(image: &ImageView<'_>, blocks: &mut [Line]) {
    for block in blocks {
        block.layout_ink_height = measure(image, block.layout_bounds);
    }
}

pub fn measure(image: &ImageView<'_>, bounds: RectF) -> Option<f64> {
    if bounds.height < MIN_BOX_HEIGHT || bounds.width < bounds.height * MIN_BOX_ASPECT {
        return None;
    }
    let left = 0.max(bounds.x.floor() as i32);
    let top = 0.max(bounds.y.floor() as i32);
    let right = (image.width as i32).min(bounds.right().ceil() as i32);
    let bottom = (image.height as i32).min(bounds.bottom().ceil() as i32);
    if right <= left || bottom <= top {
        return None;
    }
    let step = 1.max((right - left) / MAX_ROW_SAMPLES) as usize;
    let pixel = |x: i32, y: i32| -> &[u8] { &image.row(y as u32)[x as usize * 4..][..3] };
    let mut histogram = [0_i32; 4096];
    let mut total = 0_i32;
    for y in top..bottom {
        for x in (left..right).step_by(step) {
            let c = pixel(x, y);
            let bin = (usize::from(c[2] >> QUANTIZATION_SHIFT) << 8)
                | (usize::from(c[1] >> QUANTIZATION_SHIFT) << 4)
                | usize::from(c[0] >> QUANTIZATION_SHIFT);
            histogram[bin] += 1;
            total += 1;
        }
    }
    let peak_color_count = *histogram.iter().max().unwrap();
    let mode = histogram
        .iter()
        .position(|count| *count == peak_color_count)
        .unwrap();
    if f64::from(histogram[mode]) < f64::from(total) * MIN_BACKGROUND_SHARE {
        return None;
    }
    let red = (mode as i32 >> 8) * QUANTIZATION_STEP + QUANTIZATION_MIDPOINT;
    let green = ((mode as i32 >> 4) & 15) * QUANTIZATION_STEP + QUANTIZATION_MIDPOINT;
    let blue = (mode as i32 & 15) * QUANTIZATION_STEP + QUANTIZATION_MIDPOINT;
    let is_ink = |c: &[u8]| {
        (i32::from(c[2]) - red)
            .abs()
            .max((i32::from(c[1]) - green).abs())
            .max((i32::from(c[0]) - blue).abs())
            > MAX_BACKGROUND_DELTA
    };
    let (mut border, mut border_ink) = (0, 0);
    for x in (left..right).step_by(step) {
        border += 2;
        if is_ink(pixel(x, top)) {
            border_ink += 1;
        }
        if is_ink(pixel(x, bottom - 1)) {
            border_ink += 1;
        }
    }
    if f64::from(border_ink) > f64::from(border) * MAX_BORDER_INK_SHARE {
        return None;
    }
    let mut rows = vec![0; (bottom - top) as usize];
    for y in top..bottom {
        for x in (left..right).step_by(step) {
            if is_ink(pixel(x, y)) {
                rows[(y - top) as usize] += 1;
            }
        }
    }
    let peak = *rows.iter().max().unwrap();
    if peak < MIN_ROW_PEAK {
        return None;
    }
    let mut occupied = rows
        .iter()
        .enumerate()
        .filter(|(_, count)| f64::from(**count) > f64::from(peak) * MIN_ROW_PEAK_SHARE)
        .map(|(row, _)| row);
    let first = occupied.next()?;
    let last = occupied.next_back().unwrap_or(first);
    let height = (last - first + 1) as i32;
    (height >= MIN_INK_HEIGHT && f64::from(height) < bounds.height * MAX_INK_HEIGHT_SHARE)
        .then_some(f64::from(height))
}
