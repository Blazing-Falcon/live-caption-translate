//! Draft text rules: token budget, repeat
//! cleaning, rejection guards and the prefill that lets a draft continue from the previous one.
//!
//! Ported from a Python prototype; the fixtures in
//! `tests/fixtures/v2/draft-*.json` are the arbiter.

/// Drafts are short-lived: a looping model must not run on.
const DRAFT_TOKEN_CAP: usize = 64;
/// Most repeats collapsed in one text.
const CLEAN_PASSES: usize = 3;
/// Longest repeated unit, in words.
const MAX_UNIT_WORDS: usize = 4;

/// A character that carries speech: CJK, ASCII letter or digit.
fn is_word_char(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c) || c.is_ascii_alphanumeric()
}

/// Word characters in a partial; drives the draft growth rule.
pub fn word_chars(text: &str) -> usize {
    text.chars().filter(|c| is_word_char(*c)).count()
}

/// `min(64, 3 x characters + 8)`: all characters of the partial, punctuation included.
pub fn draft_max_tokens(zh: &str) -> u32 {
    (3 * zh.chars().count() + 8).min(DRAFT_TOKEN_CAP) as u32
}

fn lower(text: &str) -> String {
    text.to_lowercase()
}

fn trim_commas(text: &str) -> &str {
    text.trim_end_matches(',')
}

/// Python's `\W` complement: letters, digits and underscore.
fn is_python_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Run {
    start: usize,
    size: usize,
    end: usize,
    /// Characters of the last token that belong to the run when it ends in other punctuation.
    partial: Option<usize>,
}

fn find_run(tokens: &[&str]) -> Option<Run> {
    for i in 0..tokens.len() {
        for size in 1..=MAX_UNIT_WORDS {
            if i + size > tokens.len() {
                break;
            }
            let unit = &tokens[i..i + size];
            let head: Vec<String> = unit[..size - 1].iter().map(|t| lower(t)).collect();
            let key_last = lower(trim_commas(unit[size - 1]));
            let mut reps = 1;
            let mut j = i + size;
            let mut partial = None;
            while j + size <= tokens.len() {
                let candidate = &tokens[j..j + size];
                let same_head = candidate[..size - 1]
                    .iter()
                    .map(|t| lower(t))
                    .eq(head.iter().cloned());
                if !same_head {
                    break;
                }
                let last = lower(candidate[size - 1]);
                if trim_commas(&last) == key_last {
                    reps += 1;
                    j += size;
                    continue;
                }
                // The run's final repeat may end in other punctuation ("stars?"); the run stops
                // before that punctuation.
                if reps >= 2 && last.starts_with(&key_last) {
                    let next = last[key_last.len()..].chars().next();
                    if next.is_none_or(|c| !is_python_word(c)) {
                        reps += 1;
                        j += size;
                        partial = Some(key_last.chars().count());
                    }
                }
                break;
            }
            if reps >= 3 {
                return Some(Run {
                    start: i,
                    size,
                    end: j,
                    partial,
                });
            }
        }
    }
    None
}

fn chars_from(text: &str, skip: usize) -> String {
    text.chars().skip(skip).collect()
}

/// Collapse whitespace, then collapse each run of a repeated unit (1 to 4 words, 3 or more times,
/// separated by spaces and/or commas, case-insensitive) to one copy. Text after a run is kept
/// unless it is only a cut-off copy of the unit (a runaway that hit the token cap).
pub fn clean_draft(text: &str) -> String {
    let mut text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for _ in 0..CLEAN_PASSES {
        let tokens: Vec<&str> = if text.is_empty() {
            Vec::new()
        } else {
            text.split(' ').collect()
        };
        let Some(run) = find_run(&tokens) else {
            return text;
        };
        let unit_joined = tokens[run.start..run.start + run.size].join(" ");
        let unit_text = trim_commas(&unit_joined).to_owned();
        let last = tokens[run.end - 1];
        let after = match run.partial {
            Some(chars) => chars_from(last, chars),
            None => last[trim_commas(last).len()..].to_owned(),
        };
        let mut rest = after;
        if run.end < tokens.len() {
            rest.push(' ');
            rest.push_str(&tokens[run.end..].join(" "));
        }
        let tail = rest.trim_matches(|c| c == ' ' || c == ',');
        if !tail.is_empty() && lower(&unit_text).starts_with(&lower(tail)) {
            rest.clear();
        }
        let before = tokens[..run.start].join(" ");
        let mut next = String::new();
        if !before.is_empty() {
            next.push_str(&before);
            next.push(' ');
        }
        next.push_str(&unit_text);
        next.push_str(&rest);
        text = next.trim().to_owned();
    }
    text
}

/// Why a cleaned draft must not be published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rejection {
    Empty,
    EqualsSource,
    MostlyCjk,
}

/// Guards: empty text, text equal to the source, and text in which
/// CJK characters outnumber ASCII letters.
pub fn reject(source: &str, cleaned: &str) -> Option<Rejection> {
    if cleaned.trim().is_empty() {
        return Some(Rejection::Empty);
    }
    if cleaned.trim() == source.trim() {
        return Some(Rejection::EqualsSource);
    }
    let cjk = cleaned
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .count();
    let letters = cleaned.chars().filter(char::is_ascii_alphabetic).count();
    (cjk > letters).then_some(Rejection::MostlyCjk)
}

fn trim_end_punctuation(text: &str) -> &str {
    text.trim_end_matches(|c: char| "，。？！、,.?!".contains(c) || c.is_whitespace())
}

/// English to prefill the draft model's answer with: the previous draft of the same clause minus
/// its last `keep_back` words, used only when the new Chinese extends the previous Chinese.
/// The result ends in one space, or is empty.
pub fn prefill_prefix(
    prev_zh: Option<&str>,
    prev_en: Option<&str>,
    new_zh: &str,
    keep_back: usize,
) -> String {
    let (Some(prev_zh), Some(prev_en)) = (prev_zh, prev_en) else {
        return String::new();
    };
    if prev_zh.is_empty() || prev_en.is_empty() {
        return String::new();
    }
    let core = trim_end_punctuation(prev_zh);
    let words: Vec<&str> = prev_en.split_whitespace().collect();
    let core_chars = core.chars().count();
    let take = core_chars.saturating_sub(1).max(1).min(core_chars);
    let head: String = core.chars().take(take).collect();
    if words.len() <= keep_back || !new_zh.starts_with(&head) {
        return String::new();
    }
    let mut prefix = words[..words.len() - keep_back].join(" ");
    prefix.push(' ');
    prefix
}

/// Which words of the newest draft the overlay shows (`overlay.draft_display`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayPolicy {
    /// The newest two words are held back, but at least two words show.
    Hold2,
    /// Only the words the last two drafts agree on.
    Settled,
    All,
}

impl DisplayPolicy {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "hold2" => Some(Self::Hold2),
            "settled" => Some(Self::Settled),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

/// What the overlay shows for a line whose newest draft is `drafts.last()`. `shown` holds the
/// words on screen now and `held_once` says the previous update was suppressed by the shrink
/// rule. Returns the words to show and whether this update was held back.
///
/// The UI has its own implementation (`captions.ts`); both must satisfy
/// `tests/fixtures/v2/draft-display.json`. This one serves the replay report's rewrite counts.
pub fn visible_words(
    policy: DisplayPolicy,
    drafts: &[String],
    committed: bool,
    shown: &[String],
    held_once: bool,
) -> (Vec<String>, bool) {
    let words: Vec<String> = drafts
        .last()
        .map(|draft| draft.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let n = words.len();
    let target: Vec<String> = if committed || policy == DisplayPolicy::All {
        words
    } else if policy == DisplayPolicy::Hold2 {
        words[..n.min(2).max(n.saturating_sub(2))].to_vec()
    } else if drafts.len() < 2 {
        Vec::new()
    } else {
        let previous: Vec<&str> = drafts[drafts.len() - 2].split_whitespace().collect();
        let common = words
            .iter()
            .zip(&previous)
            .take_while(|(a, b)| a.as_str() == **b)
            .count();
        words[..common].to_vec()
    };
    // A draft that would shrink the line to less than half is held for one update.
    if !shown.is_empty() && !held_once && !committed && target.len() * 2 < shown.len() {
        return (shown.to_vec(), true);
    }
    (target, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn display_policies_match_the_recorded_sequences() {
        let list = cases(include_str!("../tests/fixtures/v2/draft-display.json"));
        assert_eq!(list.len(), 81);
        for case in &list {
            let policy = DisplayPolicy::parse(case["policy"].as_str().unwrap()).unwrap();
            let mut drafts: Vec<String> = Vec::new();
            let mut shown: Vec<String> = Vec::new();
            let mut held = false;
            for step in case["steps"].as_array().unwrap() {
                let expected = step["visible"].as_str().unwrap();
                if step.get("committed").is_some() {
                    let (words, _) = visible_words(policy, &drafts, true, &shown, held);
                    assert_eq!(words.join(" "), expected, "committed, {case}");
                    continue;
                }
                drafts.push(step["draft"].as_str().unwrap().to_owned());
                let (words, was_held) = visible_words(policy, &drafts, false, &shown, held);
                assert_eq!(words.join(" "), expected, "{:?} {:?}", policy, drafts);
                assert_eq!(was_held, step["held"].as_bool().unwrap());
                shown = words;
                held = was_held;
            }
        }
    }

    fn cases(json: &str) -> Vec<Value> {
        serde_json::from_str::<Value>(json)
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    fn text<'a>(case: &'a Value, key: &str) -> Option<&'a str> {
        case[key].as_str()
    }

    #[test]
    fn annotated_clean_cases_pass() {
        let list = cases(include_str!("../tests/fixtures/v2/draft-clean.json"));
        assert_eq!(list.len(), 131);
        for case in &list {
            let raw = text(case, "raw").unwrap();
            assert_eq!(
                clean_draft(raw),
                text(case, "expected").unwrap(),
                "raw {raw:?}"
            );
            if let (Some(zh), Some(budget)) = (text(case, "zh"), case["max_tokens_for_zh"].as_u64())
            {
                assert_eq!(u64::from(draft_max_tokens(zh)), budget, "zh {zh:?}");
            }
        }
    }

    #[test]
    fn bulk_clean_cases_pass() {
        let list = cases(include_str!("../tests/fixtures/v2/draft-clean-bulk.json"));
        assert_eq!(list.len(), 680);
        for case in &list {
            let raw = text(case, "raw").unwrap();
            assert_eq!(
                clean_draft(raw),
                text(case, "expected").unwrap(),
                "raw {raw:?}"
            );
        }
    }

    #[test]
    fn prefill_cases_pass() {
        let list = cases(include_str!("../tests/fixtures/v2/draft-prefill.json"));
        assert_eq!(list.len(), 80);
        let mut non_empty = 0;
        for case in &list {
            let new_zh = text(case, "new_zh").unwrap();
            let got = prefill_prefix(text(case, "prev_zh"), text(case, "prev_en"), new_zh, 2);
            assert_eq!(got, text(case, "expected_prefix").unwrap(), "{case}");
            assert_eq!(
                word_chars(new_zh) as u64,
                case["new_zh_word_chars"].as_u64().unwrap()
            );
            non_empty += usize::from(!got.is_empty());
        }
        assert!(non_empty > 10, "fixture exercises real prefills");
    }
}
