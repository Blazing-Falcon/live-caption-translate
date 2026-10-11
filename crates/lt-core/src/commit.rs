//! The commit rule. Pure: a clause's tokens and token start times in, one decision out.

/// Sentence-ending marks: a clause may end here when speech continues.
const END_MARKS: &str = "。？！?!";
/// Clause marks: a clause may end here once enough words precede them.
const SOFT_MARKS: &str = "，；,;";
/// Tokens that must follow a mark before it counts (shows the speaker kept going).
const MIN_TOKENS_AFTER: usize = 2;
/// Word tokens before a sentence-ending mark.
const MIN_TOKENS_BEFORE_END: usize = 2;
/// Wait cap: only look for a gap in this window before the last token.
const CAP_WINDOW_S: f64 = 2.0;
/// Wait cap: word tokens needed before the cut.
const CAP_MIN_TOKENS_BEFORE: usize = 4;
/// Cut this long before the token that starts the next clause.
pub const CUT_OFFSET_S: f64 = 0.06;
/// A committed clause is at least this long.
pub const MIN_CLAUSE_S: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitSettings {
    pub comma_min_tokens: usize,
    pub split_long: bool,
    pub cap_tokens: usize,
}

impl CommitSettings {
    pub const STREAMING: Self = Self {
        comma_min_tokens: 8,
        split_long: true,
        cap_tokens: 20,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    None,
    /// Cut after token `k` (punctuation commit).
    Commit {
        k: usize,
    },
    /// Cut after token `k` (wait cap, no punctuation).
    Cap {
        k: usize,
    },
}

/// A recognizer token containing a CJK character, an ASCII letter or a digit.
pub fn is_word_token(token: &str) -> bool {
    token
        .chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c) || c.is_ascii_alphanumeric())
}

/// True when the token is exactly one character and that character is in `set`.
fn single_char_in(token: &str, set: &str) -> bool {
    let mut chars = token.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if set.contains(c))
}

fn word_count(tokens: &[String]) -> usize {
    tokens.iter().filter(|t| is_word_token(t)).count()
}

/// Concatenate tokens the way the recognizer's text is built (English word starts carry a space).
pub fn join_tokens(tokens: &[String]) -> String {
    tokens.concat().trim().to_owned()
}

/// Decide whether the clause decoded as `tokens` (start times `times`, seconds from the clause
/// start) should be cut. The streaming recognizer's tokens never change, so a cut needs no
/// second look.
pub fn decide(tokens: &[String], times: &[f64], settings: &CommitSettings) -> Decision {
    let n = tokens.len();
    let mut candidate = None;
    // Suffix counts make the scan linear: words after k, words before k.
    let mut after = word_count(tokens);
    let mut before = 0;
    for (k, token) in tokens.iter().enumerate() {
        if is_word_token(token) {
            after -= 1;
        }
        if after >= MIN_TOKENS_AFTER {
            let is_end = single_char_in(token, END_MARKS);
            let is_soft = single_char_in(token, SOFT_MARKS);
            if (is_end && before >= MIN_TOKENS_BEFORE_END)
                || (is_soft && before >= settings.comma_min_tokens)
            {
                candidate = Some(k);
            }
        }
        if is_word_token(token) {
            before += 1;
        }
    }
    if let Some(k) = candidate {
        return Decision::Commit { k };
    }
    if settings.split_long
        && settings.cap_tokens > 0
        && n >= 4
        && times.len() == n
        && word_count(tokens) >= settings.cap_tokens
    {
        let floor = times[n - 1] - CAP_WINDOW_S;
        let mut best: Option<(f64, usize)> = None;
        let mut words_through = 0;
        for k in 0..=n - 4 {
            if is_word_token(&tokens[k]) {
                words_through += 1;
            }
            if times[k] >= floor && words_through >= CAP_MIN_TOKENS_BEFORE {
                let gap = times[k + 1] - times[k];
                if best.is_none_or(|(widest, _)| gap > widest) {
                    best = Some((gap, k));
                }
            }
        }
        if let Some((_, k)) = best {
            return Decision::Cap { k };
        }
    }
    Decision::None
}

/// Seconds from the stream origin at which to cut: just before token `k + 1` starts, at least
/// `MIN_CLAUSE_S` into the clause and no later than the decoded audio end.
pub fn cut_at(clause_start_s: f64, next_token_time_s: f64, audio_end_s: f64) -> f64 {
    (clause_start_s + next_token_time_s - CUT_OFFSET_S)
        .max(clause_start_s + MIN_CLAUSE_S)
        .min(audio_end_s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<String> {
        text.chars().map(String::from).collect()
    }

    /// Twenty words without punctuation: the cap cuts at the widest gap of the last two seconds
    /// that has four words before it.
    #[test]
    fn the_wait_cap_cuts_at_the_widest_recent_gap() {
        let words = tokens("我们今天一起去公园玩然后回家吃饭再看电影接着聊天");
        let mut times: Vec<f64> = (0..words.len()).map(|i| i as f64 * 0.2).collect();
        // A wider gap after the 17th word (index 16), inside the last two seconds.
        for time in &mut times[17..] {
            *time += 0.3;
        }
        assert_eq!(
            decide(&words, &times, &CommitSettings::STREAMING),
            Decision::Cap { k: 16 }
        );
        let off = CommitSettings {
            split_long: false,
            ..CommitSettings::STREAMING
        };
        assert_eq!(decide(&words, &times, &off), Decision::None);
    }
}
