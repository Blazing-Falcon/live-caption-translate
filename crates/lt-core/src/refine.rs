//! The second pass's text for a closed clause.
//!
//! The offline recognizer reads a window with a margin on both sides of the clause, so its
//! text can start with the end of the clause before and end with the start of the next one.
//! The live text decides which part belongs to the clause: token times would not, because a
//! streaming model reports each word late and its cuts land late.

/// A chosen text that keeps fewer of the live characters, or that differs from the live text in
/// more of them, is more likely a misaligned window than a correction.
const MIN_KEPT: f64 = 0.6;
const MAX_DISTANCE: f64 = 0.6;

#[derive(Clone, Debug, PartialEq)]
pub struct Trimmed {
    /// The part of the offline text that matches the clause, with its punctuation.
    pub text: String,
    /// Edit distance to the live text over letters and digits.
    pub distance: usize,
}

/// A letter or digit of `text` (lowercase), with its byte range.
struct Unit {
    character: char,
    start: usize,
    end: usize,
}

fn units(text: &str) -> Vec<Unit> {
    text.char_indices()
        .filter(|(_, character)| character.is_alphanumeric())
        .map(|(start, character)| Unit {
            character: character.to_lowercase().next().unwrap_or(character),
            start,
            end: start + character.len_utf8(),
        })
        .collect()
}

fn is_mark(character: char) -> bool {
    !character.is_alphanumeric() && !character.is_whitespace()
}

/// A mark that opens a quote or a bracket belongs to the words after it.
fn is_opening(character: char) -> bool {
    matches!(
        character,
        '“' | '‘' | '「' | '『' | '（' | '(' | '《' | '〈' | '【' | '['
    )
}

/// The part of `offline` that best matches `live`, or `None` when no part is close enough.
///
/// The match ignores punctuation and case and may skip offline characters at both ends. Of
/// equally close parts, the one that starts and ends at punctuation wins, then the one closest
/// to the live length: a wrong first or last live character would otherwise tie with a part
/// that leaves out the corrected character.
pub fn trim(live: &str, offline: &str) -> Option<Trimmed> {
    let live_units: Vec<char> = units(live).iter().map(|unit| unit.character).collect();
    let offline_units = units(offline);
    let n = live_units.len();
    let m = offline_units.len();
    if n == 0 || m == 0 {
        return None;
    }
    // Text between two offline units holds a mark: a clause boundary in the offline text.
    let boundary_before = |index: usize| {
        index == 0
            || offline[offline_units[index - 1].end..offline_units[index].start]
                .chars()
                .any(is_mark)
    };
    let boundary_after = |index: usize| index == m || boundary_before(index);
    // Edit distance of the live text to each part offline[i..j], one start at a time.
    let mut best: Option<(usize, u8, usize, usize, usize)> = None;
    let mut row: Vec<usize> = Vec::with_capacity(n + 1);
    let mut next: Vec<usize> = vec![0; n + 1];
    for i in 0..m {
        row.clear();
        row.extend(0..=n);
        for (j, unit) in offline_units.iter().enumerate().skip(i) {
            let character = unit.character;
            next[0] = row[0] + 1;
            for k in 1..=n {
                next[k] = (row[k] + 1)
                    .min(next[k - 1] + 1)
                    .min(row[k - 1] + usize::from(live_units[k - 1] != character));
            }
            std::mem::swap(&mut row, &mut next);
            let end = j + 1;
            let edges = u8::from(boundary_before(i)) + u8::from(boundary_after(end));
            let candidate = (row[n], 2 - edges, (end - i).abs_diff(n), i, end);
            if best.is_none_or(|best| candidate < best) {
                best = Some(candidate);
            }
        }
    }
    let (distance, _, _, start, end) = best?;
    let kept = end - start;
    if (kept as f64) < MIN_KEPT * n as f64 || distance as f64 > MAX_DISTANCE * n as f64 {
        return None;
    }
    // The marks right after the last unit belong to the clause, up to one that opens the next
    // words; an opening mark right before the first unit belongs to it.
    let tail_end = offline[offline_units[end - 1].end..]
        .char_indices()
        .find(|(_, character)| !is_mark(*character) || is_opening(*character))
        .map_or(offline.len(), |(offset, _)| {
            offline_units[end - 1].end + offset
        });
    let head = offline[..offline_units[start].start]
        .chars()
        .next_back()
        .filter(|character| is_opening(*character))
        .map_or(0, char::len_utf8);
    let mut text = offline[offline_units[start].start - head..tail_end].to_owned();
    // A commit cut ends at a mark; keep it when the offline text has none there.
    if !text.ends_with(is_mark) {
        let live_tail = live.trim_end();
        let marks = live_tail.len() - live_tail.trim_end_matches(is_mark).len();
        text.push_str(&live_tail[live_tail.len() - marks..]);
    }
    Some(Trimmed { text, distance })
}
