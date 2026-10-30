//! Text recognition: each detected rect, resized to the recognizer's height, read by greedy CTC.

use lt_core::screen::raster::Raster;

pub(crate) const HEIGHT: u32 = 48;
pub(crate) const MIN_SCORE: f32 = 0.5;

/// One entry per line. Lines are kept exactly: one of them is U+3000, and trimming it would
/// shift every later index.
pub(crate) fn dictionary(text: &str) -> Vec<String> {
    let mut entries: Vec<String> = text.split('\n').map(str::to_owned).collect();
    if entries.last().is_some_and(String::is_empty) {
        entries.pop();
    }
    entries
}

pub(crate) fn width(crop: &Raster) -> u32 {
    let scale = HEIGHT as f32 / crop.height as f32;
    (crop.width as f32 * scale) as u32
}

pub(crate) fn input(crop: &Raster) -> Vec<f32> {
    let width = width(crop);
    let resized = Raster::resize(&crop.view(), width, HEIGHT);
    let plane = HEIGHT as usize * width as usize;
    let mut tensor = vec![0.0; 3 * plane];
    const NORM: f32 = 1.0 / 127.5;
    for (i, pixel) in resized.bgra.as_chunks::<4>().0.iter().enumerate() {
        for channel in 0..3 {
            tensor[channel * plane + i] = (f32::from(pixel[channel]) - 127.5) * NORM;
        }
    }
    tensor
}

/// Greedy CTC over `steps` (time x classes probabilities). Class 0 is the blank, class i is
/// dictionary entry i - 1, and the class after the dictionary is a space. Returns the text and
/// the mean probability and individual probabilities of the kept steps.
pub(crate) fn decode(
    steps: &[f32],
    classes: usize,
    dictionary: &[String],
) -> (String, f32, Vec<f32>) {
    let mut text = String::new();
    let mut scores = Vec::new();
    let mut previous = 0;
    for step in steps.chunks_exact(classes) {
        let (best, probability) =
            step.iter()
                .copied()
                .enumerate()
                .fold((0, -1000.0), |a, b| if b.1 > a.1 { b } else { a });
        if best != 0 && best != previous {
            text.push_str(dictionary.get(best - 1).map_or(" ", String::as_str));
            scores.push(probability);
        }
        previous = best;
    }
    let score = if scores.is_empty() {
        0.0
    } else {
        (scores.iter().map(|score| f64::from(*score)).sum::<f64>() / scores.len() as f64) as f32
    };
    (text, score, scores)
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
        assert_eq!(
            decode(&steps, 6, &entries),
            ("cc a".to_owned(), 1.0, vec![1.0; 4])
        );
    }
}
