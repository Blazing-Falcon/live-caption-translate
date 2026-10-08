//! Cleanup, script routing and conservative false-trigger filtering.

use crate::config::FilterConfig;
use crate::events::DropReason;
use crate::types::{TextClass, Transcript};

/// Remove engine markup and standalone fillers while preserving spoken words.
///
/// A Chinese filler is standalone only when neither neighbor is a Chinese
/// character. English fillers use word boundaries and ignore ASCII case.
/// Repeated characters within speech remain untouched.
pub fn clean(text: &str, fillers: &[String]) -> String {
    let stripped = strip_engine_tokens(text);
    let characters: Vec<char> = stripped.chars().collect();
    let mut fillers: Vec<Vec<char>> = fillers
        .iter()
        .filter(|filler| !filler.is_empty())
        .map(|filler| filler.chars().collect())
        .collect();
    // Prefer a configured phrase over its shorter prefix at the same position.
    fillers.sort_by_key(|filler| std::cmp::Reverse(filler.len()));

    let mut without_fillers = String::with_capacity(stripped.len());
    let mut index = 0;
    while index < characters.len() {
        let matched = fillers.iter().find(|filler| {
            let end = index + filler.len();
            end <= characters.len()
                && characters[index..end]
                    .iter()
                    .zip(filler.iter())
                    .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
                && standalone(&characters, index, end, filler)
        });
        if let Some(filler) = matched {
            index += filler.len();
        } else {
            without_fillers.push(characters[index]);
            index += 1;
        }
    }

    normalize_punctuation(&without_fillers)
}

/// Classify script evidence; punctuation, digits and isolated Latin letters
/// do not establish a class by themselves.
pub fn classify(text: &str) -> Option<TextClass> {
    classify_with_lang(text, None)
}

/// Use a language tag only to resolve otherwise inconclusive text evidence.
/// A single Latin letter may be English, but a tag cannot override Chinese,
/// mixed text, or dominant kana/Hangul.
pub fn classify_with_lang(text: &str, lang_tag: Option<&str>) -> Option<TextClass> {
    let chinese = chinese_chars(text);
    let latin = latin_words(text);
    let other = text
        .chars()
        .filter(|character| is_kana_or_hangul(*character) && !is_punctuation(*character))
        .count();
    if other > chinese {
        Some(TextClass::Other)
    } else if chinese > 0 && latin > 0 {
        Some(TextClass::Mixed)
    } else if chinese > 0 {
        Some(TextClass::Chinese)
    } else if latin > 0
        || (lang_tag.is_some_and(|tag| normalized_tag(tag).eq_ignore_ascii_case("en"))
            && text
                .chars()
                .any(|character| character.is_ascii_alphabetic()))
    {
        Some(TextClass::English)
    } else {
        None
    }
}

/// Count the two ideograph ranges specified for Chinese routing and joining.
pub fn chinese_chars(text: &str) -> usize {
    text.chars()
        .filter(|character| is_chinese(*character))
        .count()
}

/// Count contiguous ASCII letter runs of at least two letters, including
/// Latin words attached directly to Chinese text.
pub fn latin_words(text: &str) -> usize {
    let mut words = 0;
    let mut length = 0;
    for character in text.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_alphabetic() {
            length += 1;
        } else {
            if length >= 2 {
                words += 1;
            }
            length = 0;
        }
    }
    words
}

/// Apply the rules in order: empty text, music without enough word evidence, then
/// brief single-character false triggers. The input may still contain markup
/// or fillers; decisions always use its cleaned text.
pub fn drop_reason(
    transcript: &Transcript,
    duration_s: f64,
    config: &FilterConfig,
) -> Option<DropReason> {
    let text = clean(&transcript.text, &config.fillers);
    if classify_with_lang(&text, transcript.lang_tag.as_deref()).is_none() {
        return Some(DropReason::Empty);
    }

    if config.drop_music
        && transcript
            .event
            .as_deref()
            .is_some_and(|event| normalized_tag(event).eq_ignore_ascii_case("BGM"))
        && chinese_chars(&text) < 2
        && latin_words(&text) == 0
    {
        return Some(DropReason::Music);
    }

    let mut spoken_characters = text
        .chars()
        .filter(|character| !character.is_whitespace() && !is_punctuation(*character));
    if let Some(character) = spoken_characters.next() {
        // Compare at the config's f32 precision so the literal 0.6 s boundary
        // is not treated as shorter because f32(0.6) rounds upward.
        if spoken_characters.next().is_none()
            && (duration_s as f32) < config.single_char_max_s
            && !config.single_char_allow.contains(&character.to_string())
        {
            return Some(DropReason::SingleChar);
        }
    }
    None
}

fn is_chinese(character: char) -> bool {
    matches!(character, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}')
}

fn is_kana_or_hangul(character: char) -> bool {
    matches!(character, '\u{3040}'..='\u{30ff}' | '\u{ac00}'..='\u{d7af}')
}

fn normalized_tag(tag: &str) -> &str {
    let tag = tag.trim();
    tag.strip_prefix("<|")
        .and_then(|tag| tag.strip_suffix("|>"))
        .unwrap_or(tag)
}

fn strip_engine_tokens(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    while !remaining.is_empty() {
        if let Some(after) = remaining.strip_prefix("[UNK]") {
            remaining = after;
        } else if let Some(tag) = remaining.strip_prefix("<|") {
            if let Some(end) = tag.find("|>") {
                remaining = &tag[end + 2..];
            } else if let Some(character) = remaining.chars().next() {
                result.push(character);
                remaining = &remaining[character.len_utf8()..];
            }
        } else if let Some(character) = remaining.chars().next() {
            result.push(character);
            remaining = &remaining[character.len_utf8()..];
        }
    }
    result
}

fn standalone(characters: &[char], start: usize, end: usize, filler: &[char]) -> bool {
    let boundary = if filler.iter().any(|character| is_chinese(*character)) {
        is_chinese as fn(char) -> bool
    } else {
        is_word_character as fn(char) -> bool
    };
    (start == 0 || !boundary(characters[start - 1]))
        && (end == characters.len() || !boundary(characters[end]))
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn is_sentence_punctuation(character: char) -> bool {
    matches!(
        character,
        ',' | '.' | '!' | '?' | ';' | ':' | '，' | '。' | '！' | '？' | '；' | '：' | '、' | '…'
    )
}

fn is_punctuation(character: char) -> bool {
    character.is_ascii_punctuation()
        || matches!(character, '\u{2000}'..='\u{206f}' | '\u{3001}'..='\u{303f}' | '\u{309b}' | '\u{309c}' | '\u{30a0}' | '\u{30fb}' | '\u{fe10}'..='\u{fe1f}' | '\u{fe30}'..='\u{fe4f}' | '\u{ff01}'..='\u{ff0f}' | '\u{ff1a}'..='\u{ff20}' | '\u{ff3b}'..='\u{ff40}' | '\u{ff5b}'..='\u{ff65}')
}

fn normalize_punctuation(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut punctuation = None;
    let mut space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            space = !result.is_empty();
        } else if result.is_empty() && is_punctuation(character) {
            // Punctuation left after a leading filler carries no words.
        } else if is_sentence_punctuation(character) {
            // Keep the final mark of a run: a removed filler in "，嗯。"
            // leaves a full stop rather than an orphaned comma/full stop pair.
            punctuation = Some(character);
            space = false;
        } else {
            if let Some(punctuation) = punctuation.take() {
                result.push(punctuation);
            }
            if space {
                result.push(' ');
            }
            result.push(character);
            space = false;
        }
    }
    if let Some(punctuation) = punctuation {
        result.push(punctuation);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StageTiming, UtteranceId};

    #[test]
    fn cleanup_and_routing_table() {
        let config = FilterConfig::default();
        let cases = [
            (
                "也不是也不完全是AI。",
                "也不是也不完全是AI。",
                Some(TextClass::Mixed),
            ),
            (
                "veification code输进去之后就可以。",
                "veification code输进去之后就可以。",
                Some(TextClass::Mixed),
            ),
            (
                "呃，你今天想要穿什么衣服",
                "你今天想要穿什么衣服",
                Some(TextClass::Chinese),
            ),
            (
                "不能去desecide呃，你今天…",
                "不能去desecide，你今天…",
                Some(TextClass::Mixed),
            ),
            ("我们看看吧。", "我们看看吧。", Some(TextClass::Chinese)),
            ("谢谢谢谢。", "谢谢谢谢。", Some(TextClass::Chinese)),
            (
                "Okay I see, that makes sense.",
                "Okay I see, that makes sense.",
                Some(TextClass::English),
            ),
            (
                "그it card 어oc 가 쓸정 없이.",
                "그it card 어oc 가 쓸정 없이.",
                Some(TextClass::Other),
            ),
            (
                "こんにちは、元気ですか。",
                "こんにちは、元気ですか。",
                Some(TextClass::Other),
            ),
            ("嗯。", "", None),
            (
                "<|zh|><|NEUTRAL|><|Speech|>好的。",
                "好的。",
                Some(TextClass::Chinese),
            ),
            ("这个[UNK]东西", "这个东西", Some(TextClass::Chinese)),
            ("123。", "123。", None),
            ("呃你说吧。", "呃你说吧。", Some(TextClass::Chinese)),
            (
                "我嗯嗯觉得可以。",
                "我嗯嗯觉得可以。",
                Some(TextClass::Chinese),
            ),
            ("你好，嗯，世界。", "你好，世界。", Some(TextClass::Chinese)),
            ("你好，额。世界。", "你好。世界。", Some(TextClass::Chinese)),
            (" ，呃，， 嗯。 ", "", None),
            (
                " UH, hello UM there. ",
                "hello there.",
                Some(TextClass::English),
            ),
            (
                "huh umbrella uh_oh uh2",
                "huh umbrella uh_oh uh2",
                Some(TextClass::English),
            ),
            ("你好uh世界", "你好uh世界", Some(TextClass::Mixed)),
            ("你好  world", "你好 world", Some(TextClass::Mixed)),
            ("。！？", "", None),
            ("I", "I", None),
        ];
        for (input, expected_clean, expected_class) in cases {
            let cleaned = clean(input, &config.fillers);
            assert_eq!(cleaned, expected_clean, "cleanup: {input:?}");
            assert_eq!(classify(&cleaned), expected_class, "routing: {input:?}");
        }
    }

    #[test]
    fn configurable_fillers_keep_adjacent_words() {
        let fillers = ["那个", "you know"].map(String::from);
        let cases = [
            ("那个，你好", "你好"),
            ("那个东西", "那个东西"),
            ("我是那个", "我是那个"),
            ("YOU KNOW, hello", "hello"),
            ("hello you know world", "hello world"),
            ("you knowable", "you knowable"),
            ("嗯，你好", "嗯，你好"),
        ];
        for (input, expected) in cases {
            assert_eq!(clean(input, &fillers), expected, "{input:?}");
        }
    }

    #[test]
    fn script_ranges_counts_and_tag_ties() {
        let cases = [
            ("中AI文hello1world", None, 2, 3, Some(TextClass::Mixed)),
            (
                "\u{3400}\u{4dbf}\u{4e00}\u{9fff}",
                None,
                4,
                0,
                Some(TextClass::Chinese),
            ),
            ("\u{4dc0}\u{a000}\u{20000}", None, 0, 0, None),
            ("中あ", None, 1, 0, Some(TextClass::Chinese)),
            ("中あア", Some("zh"), 1, 0, Some(TextClass::Other)),
            ("中가힣", Some("en"), 1, 0, Some(TextClass::Other)),
            ("あア가힣AI", None, 0, 1, Some(TextClass::Other)),
            ("你I", Some("en"), 1, 0, Some(TextClass::Chinese)),
            ("I", Some("en"), 0, 0, Some(TextClass::English)),
            ("I.", Some("<|EN|>"), 0, 0, Some(TextClass::English)),
            ("I", Some("zh"), 0, 0, None),
            ("123，。", Some("en"), 0, 0, None),
            ("", Some("en"), 0, 0, None),
            ("a b I 12", None, 0, 0, None),
        ];
        for (text, tag, expected_chinese, expected_latin, expected_class) in cases {
            assert_eq!(chinese_chars(text), expected_chinese, "{text:?}");
            assert_eq!(latin_words(text), expected_latin, "{text:?}");
            assert_eq!(
                classify_with_lang(text, tag),
                expected_class,
                "{text:?}, {tag:?}"
            );
        }
    }

    #[test]
    fn all_recorded_asr_fixture_outputs() {
        // Frozen sv2024_auto outputs from asr-expected.json, embedded here so
        // tests never depend on files outside the repository.
        let config = FilterConfig::default();
        let cases = [
            ("也不是也不完全是AI。", "也不是也不完全是AI。", TextClass::Mixed),
            ("veification code输进去之后就可以。", "veification code输进去之后就可以。", TextClass::Mixed),
            ("不能去desecide呃，你今天想要穿什么衣服，明天想要穿什么衣服，每天都穿一样。", "不能去desecide，你今天想要穿什么衣服，明天想要穿什么衣服，每天都穿一样。", TextClass::Mixed),
            ("初中从小学，我不知道你们是初中跟小学有没有uni。", "初中从小学，我不知道你们是初中跟小学有没有uni。", TextClass::Mixed),
            ("It's not what I expected in the beginning, but just对最后就回到这里。", "It's not what I expected in the beginning, but just对最后就回到这里。", TextClass::Mixed),
            ("对，因为就是mine他不会思考，我们很难去，它是像像一个black box。", "对，因为就是mine他不会思考，我们很难去，它是像像一个black box。", TextClass::Mixed),
            ("对我同意就是self driving car，它是一个非常high standard的那个tenic。", "对我同意就是self driving car，它是一个非常high standard的那个tenic。", TextClass::Mixed),
            ("你们意思是我们讲的是pricy的问题。", "你们意思是我们讲的是pricy的问题。", TextClass::Mixed),
            ("哦，我我在UG的时候念的是electric engineering。", "哦，我我在UG的时候念的是electric engineering。", TextClass::Mixed),
            ("对，因为就是很经常。就是我们早上上午有一个lectture刚上完，然后中午去听一个让 talk。", "对，因为就是很经常。就是我们早上上午有一个lectture刚上完，然后中午去听一个让 talk。", TextClass::Mixed),
            ("没有吗？那可能business school有的吧，就会让我们穿。", "没有吗？那可能business school有的吧，就会让我们穿。", TextClass::Mixed),
            ("그it card 어oc 가 쓸정 없이.", "그it card 어oc 가 쓸정 없이.", TextClass::Other),
            ("比较容易学的是swim。", "比较容易学的是swim。", TextClass::Mixed),
            ("啊，你是说你有一些就你最喜欢的sport是什么？", "啊，你是说你有一些就你最喜欢的sport是什么？", TextClass::Mixed),
            ("那图像识别其实上学上个ma我们。", "那图像识别其实上学上个ma我们。", TextClass::Mixed),
            ("那你现在住在 Hong港大概住了多久？", "那你现在住在 Hong港大概住了多久？", TextClass::Mixed),
            ("的的那种culture。", "的的那种culture。", TextClass::Mixed),
            ("会关系到一个paenger的一个性命嗯。", "会关系到一个paenger的一个性命嗯。", TextClass::Mixed),
            ("那你现在会不会去G打那个地球？", "那你现在会不会去G打那个地球？", TextClass::Chinese),
            ("对我也做过一些generation taskask。", "对我也做过一些generation taskask。", TextClass::Mixed),
            ("哦，那个是那个是wind，好像是做。", "哦，那个是那个是wind，好像是做。", TextClass::Mixed),
            ("然后高不不高高三是我最胖的时候 because I do not work around and just stay.", "然后高不不高高三是我最胖的时候 because I do not work around and just stay.", TextClass::Mixed),
            ("其实呢badminton有一个就是badminton，因为basketball的话主要是。", "其实呢badminton有一个就是badminton，因为basketball的话主要是。", TextClass::Mixed),
            ("activities different kind of activities对，有时候去忙一些那种舞会啊，party啊，有时候忙一些应对啊，就是low summer camp for students. I have lots of different kinds of。", "activities different kind of activities对，有时候去忙一些那种舞会啊，party啊，有时候忙一些应对啊，就是low summer camp for students. I have lots of different kinds of。", TextClass::Mixed),
            ("大四上学期好像是反正好像是6月份的时候才开始准备I。", "大四上学期好像是反正好像是6月份的时候才开始准备I。", TextClass::Chinese),
        ];
        for (input, expected_clean, expected_class) in cases {
            let cleaned = clean(input, &config.fillers);
            assert_eq!(cleaned, expected_clean, "{input:?}");
            assert_eq!(classify(&cleaned), Some(expected_class), "{input:?}");
        }
    }

    #[test]
    fn d7_drop_precedence_and_duration_table() {
        let config = FilterConfig::default();
        let cases = [
            ("", None, None, 1.0, Some(DropReason::Empty)),
            ("123。", Some("en"), None, 1.0, Some(DropReason::Empty)),
            (
                "呃，嗯。",
                None,
                Some("<|BGM|>"),
                0.4,
                Some(DropReason::Empty),
            ),
            (
                "<|zh|><|Speech|>[UNK]嗯。",
                None,
                None,
                0.4,
                Some(DropReason::Empty),
            ),
            ("对。", None, None, 0.4, None),
            ("好。", None, None, 0.4, None),
            ("是。", None, None, 0.4, None),
            ("行。", None, None, 0.4, None),
            ("不。", None, None, 0.4, None),
            ("哦。", None, None, 0.4, None),
            ("那。", None, None, 0.4, Some(DropReason::SingleChar)),
            ("那。", None, None, 0.6, None),
            ("那。", None, None, 0.8, None),
            ("那2。", None, None, 0.4, None),
            ("啦。", None, Some("BGM"), 0.4, Some(DropReason::Music)),
            // Two Chinese
            // characters are sufficient word evidence even in BGM.
            ("啦啦。", None, Some("<|BGM|>"), 0.4, None),
            ("我们今天来聊一下", None, Some("BGM"), 1.0, None),
            ("hello", None, Some("bgm"), 1.0, None),
            ("が。", None, Some("BGM"), 1.0, Some(DropReason::Music)),
            ("I", Some("en"), None, 0.4, Some(DropReason::SingleChar)),
            ("I", Some("en"), None, 1.0, None),
            ("I", None, None, 1.0, Some(DropReason::Empty)),
        ];
        for (text, lang, event, duration, expected) in cases {
            assert_eq!(
                drop_reason(&transcript(text, lang, event), duration, &config),
                expected,
                "{text:?}, {event:?}, {duration}"
            );
        }
    }

    #[test]
    fn configurable_drop_rules_table() {
        let cases = [
            (
                FilterConfig {
                    drop_music: false,
                    ..FilterConfig::default()
                },
                "啦。",
                1.0,
                None,
            ),
            (
                FilterConfig {
                    drop_music: false,
                    ..FilterConfig::default()
                },
                "啦。",
                0.4,
                Some(DropReason::SingleChar),
            ),
            (
                FilterConfig {
                    single_char_allow: vec!["那".into()],
                    ..FilterConfig::default()
                },
                "那。",
                0.4,
                None,
            ),
            (
                FilterConfig {
                    single_char_max_s: 0.0,
                    ..FilterConfig::default()
                },
                "那。",
                0.4,
                None,
            ),
            (
                FilterConfig {
                    fillers: Vec::new(),
                    ..FilterConfig::default()
                },
                "嗯。",
                1.0,
                None,
            ),
        ];
        for (config, text, duration, expected) in cases {
            let event = if text == "啦。" { Some("BGM") } else { None };
            assert_eq!(
                drop_reason(&transcript(text, None, event), duration, &config),
                expected,
                "{text:?}, {duration}"
            );
        }
    }

    fn transcript(text: &str, lang: Option<&str>, event: Option<&str>) -> Transcript {
        Transcript {
            id: UtteranceId(1),
            text: text.into(),
            lang_tag: lang.map(String::from),
            class: TextClass::Chinese,
            event: event.map(String::from),
            timing: StageTiming::default(),
            absorbed: Vec::new(),
            cut: crate::types::CutReason::Pause,
        }
    }
}
