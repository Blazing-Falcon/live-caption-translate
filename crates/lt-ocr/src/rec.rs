//! Text recognition: each detected rect, resized to the recognizer's height, read by greedy CTC.

use crate::image::resize;
use lt_core::screen::{ImageView, Rect};

pub(crate) const HEIGHT: u32 = 48;
const MIN_WIDTH: u32 = 16;
const MAX_WIDTH: u32 = 3072;
pub(crate) const BATCH: usize = 8;
pub(crate) const MIN_SCORE: f32 = 0.6;

/// One entry per line. Lines are kept exactly: one of them is U+3000, and trimming it would
/// shift every later index.
pub(crate) fn dictionary(text: &str) -> Vec<String> {
    let mut entries: Vec<String> = text.split('\n').map(str::to_owned).collect();
    if entries.last().is_some_and(String::is_empty) {
        entries.pop();
    }
    entries
}

pub(crate) fn width(rect: &Rect) -> u32 {
    (HEIGHT as u64 * rect.w as u64)
        .div_ceil(rect.h as u64)
        .clamp(MIN_WIDTH as u64, MAX_WIDTH as u64) as u32
}

/// Writes one crop into its slot of an NCHW batch `batch_width` wide, normalized to -1..1.
/// The rest of the slot stays 0.
pub(crate) fn fill(image: &ImageView<'_>, rect: &Rect, batch_width: u32, slot: &mut [f32]) {
    let width = width(rect);
    let pixels = resize(image, *rect, width, HEIGHT);
    let (width, batch_width) = (width as usize, batch_width as usize);
    let plane = HEIGHT as usize * batch_width;
    for y in 0..HEIGHT as usize {
        for x in 0..width {
            let pixel = &pixels[(y * width + x) * 3..][..3];
            for (channel, value) in pixel.iter().enumerate() {
                slot[channel * plane + y * batch_width + x] = value / 127.5 - 1.0;
            }
        }
    }
}

/// Greedy CTC over `steps` (time x classes probabilities). Class 0 is the blank, class i is
/// dictionary entry i - 1, and the class after the dictionary is a space. Returns the text and
/// the mean probability of the kept steps.
pub(crate) fn decode(steps: &[f32], classes: usize, dictionary: &[String]) -> (String, f32) {
    let mut text = String::new();
    let (mut sum, mut kept) = (0.0_f32, 0_u32);
    let mut previous = 0;
    for step in steps.chunks_exact(classes) {
        let (best, probability) = step
            .iter()
            .copied()
            .enumerate()
            .fold((0, f32::MIN), |a, b| if b.1 > a.1 { b } else { a });
        if best != 0 && best != previous {
            text.push_str(dictionary.get(best - 1).map_or(" ", String::as_str));
            sum += probability;
            kept += 1;
        }
        previous = best;
    }
    let score = if kept == 0 { 0.0 } else { sum / kept as f32 };
    (text, score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_keeps_whitespace_entries_so_indexes_do_not_shift() {
        let entries = dictionary("a\n\u{3000}\n b\nc\n");
        assert_eq!(entries, ["a", "\u{3000}", " b", "c"]);
        let one_hot = |class: usize| {
            let mut step = vec![0.0; 6];
            step[class] = 1.0;
            step
        };
        let steps: Vec<f32> = [4, 4, 0, 4, 5, 1].into_iter().flat_map(one_hot).collect();
        assert_eq!(decode(&steps, 6, &entries), ("cc a".to_owned(), 1.0));
    }
}
