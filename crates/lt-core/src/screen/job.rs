//! One screen job: read the text, group it, translate each block.

use super::{
    filters, group, ink,
    line::{Line, RawLine},
    ImageView, ScreenEvent,
};
use crate::{
    engines::{TextRecognizer, TranslateRequest, TranslationControl, Translator, REQUEST_TIMEOUT},
    error::Result,
    types::UtteranceId,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

/// The longest source text of one translation request, in characters.
pub const PIECE_CHARS: usize = 120;
const CACHE_ENTRIES: usize = 512;
const SENTENCE_ENDS: [char; 6] = ['。', '！', '？', '；', '…', '\n'];
const CLAUSE_ENDS: [char; 2] = ['，', '、'];
const CLOSERS: [char; 6] = ['”', '’', '」', '』', '）', ')'];

/// English for source text already translated, so a label that appears many times on a screen
/// is translated once.
#[derive(Default)]
pub struct TranslationCache {
    entries: HashMap<String, String>,
}

impl TranslationCache {
    fn insert(&mut self, source: String, english: String) {
        if self.entries.len() >= CACHE_ENTRIES {
            self.entries.clear();
        }
        self.entries.insert(source, english);
    }
}

/// Reads `image`, groups its text into blocks and translates them one request at a time. The
/// events follow the order described on [`ScreenEvent`]. Once `cancel` is set the job sends no
/// further request and emits nothing more. A block whose piece fails is reported as failed and
/// the next block still runs.
pub fn run_job(
    job: u64,
    image: &ImageView<'_>,
    recognizer: &mut dyn TextRecognizer,
    translator: &mut dyn FnMut(&str) -> Result<String>,
    cancel: &AtomicBool,
    cache: &mut TranslationCache,
    emit: &mut dyn FnMut(ScreenEvent),
) {
    run_job_with_output(
        job,
        image,
        recognizer,
        translator,
        cancel,
        cache,
        JobOutput {
            emit,
            prepare: None,
            debug: false,
        },
    );
}

pub type PreparePaint<'a> = dyn FnMut(&ImageView<'_>, &[Line], &[super::Block]) -> Result<()> + 'a;
pub struct JobOutput<'a> {
    pub debug: bool,
    pub emit: &'a mut dyn FnMut(ScreenEvent),
    pub prepare: Option<&'a mut PreparePaint<'a>>,
}

pub fn run_job_with_output(
    job: u64,
    image: &ImageView<'_>,
    recognizer: &mut dyn TextRecognizer,
    translator: &mut dyn FnMut(&str) -> Result<String>,
    cancel: &AtomicBool,
    cache: &mut TranslationCache,
    output: JobOutput<'_>,
) {
    let JobOutput {
        emit,
        prepare,
        debug,
    } = output;
    let started = Instant::now();
    let cancelled = || cancel.load(Ordering::Acquire);
    if cancelled() {
        return;
    }
    emit(ScreenEvent::Reading { job });
    let lines = match recognizer.recognize(image) {
        Ok(lines) => lines,
        Err(error) => {
            if !cancelled() {
                emit(ScreenEvent::Error {
                    job,
                    message: error.to_string(),
                });
            }
            return;
        }
    };
    if cancelled() {
        return;
    }
    let raw: Vec<RawLine> = lines.iter().map(RawLine::from).collect();
    let mut facts = filters::prepare(&raw);
    ink::annotate(image, &mut facts);
    let groups = group::group(&facts);
    let blocks = group::blocks_from_groups(image, &groups);
    if let Some(prepare) = prepare {
        if let Err(error) = prepare(image, &groups, &blocks) {
            if !cancelled() {
                emit(ScreenEvent::Error {
                    job,
                    message: error.to_string(),
                });
            }
            return;
        }
    }
    if cancelled() {
        return;
    }
    let sources: Vec<(u32, String)> = blocks
        .iter()
        .filter(|block| block.translate)
        .map(|block| (block.id, block.source.clone()))
        .collect();
    emit(ScreenEvent::Blocks {
        job,
        blocks,
        ocr_ms: millis(started),
        ocr_lines: debug.then(|| lines.iter().map(|line| line.rect).collect()),
    });
    for (id, source) in sources {
        let event = match translate_block(&source, translator, cancel, cache) {
            Outcome::Cancelled => return,
            Outcome::Translated(text) => ScreenEvent::Translated { job, id, text },
            Outcome::Failed(message) => ScreenEvent::Failed { job, id, message },
        };
        emit(event);
    }
    if cancelled() {
        return;
    }
    emit(ScreenEvent::Done {
        job,
        total_ms: millis(started),
    });
}

enum Outcome {
    Cancelled,
    Translated(String),
    Failed(String),
}

fn translate_block(
    source: &str,
    translator: &mut dyn FnMut(&str) -> Result<String>,
    cancel: &AtomicBool,
    cache: &mut TranslationCache,
) -> Outcome {
    let mut english: Vec<String> = Vec::new();
    for piece in pieces(source, PIECE_CHARS) {
        if cancel.load(Ordering::Acquire) {
            return Outcome::Cancelled;
        }
        if let Some(known) = cache.entries.get(&piece) {
            english.push(known.clone());
            continue;
        }
        let result = translator(&piece);
        if cancel.load(Ordering::Acquire) {
            return Outcome::Cancelled;
        }
        match result.map(|text| text.trim().to_owned()) {
            Ok(text) if text.is_empty() => {
                return Outcome::Failed("The translation is empty".into())
            }
            Ok(text) => {
                cache.insert(piece, text.clone());
                english.push(text);
            }
            Err(error) => return Outcome::Failed(error.to_string()),
        }
    }
    Outcome::Translated(english.join(" "))
}

fn millis(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// Adapts a [`Translator`] to the closure [`run_job`] takes. Each piece gets its own deadline
/// and stops when `cancelled` is set.
pub fn piece_translator<'a>(
    translator: &'a mut dyn Translator,
    cancelled: &Arc<AtomicBool>,
) -> impl FnMut(&str) -> Result<String> + 'a {
    let cancelled = cancelled.clone();
    let mut requests = 0;
    move |piece| {
        requests += 1;
        let request = TranslateRequest {
            id: UtteranceId(requests),
            text: piece,
            max_tokens: None,
            control: TranslationControl {
                deadline: Instant::now() + REQUEST_TIMEOUT,
                cancelled: cancelled.clone(),
                abort: Arc::new(AtomicBool::new(false)),
            },
        };
        Ok(translator.translate(&request, &mut |_| {})?.text)
    }
}

/// `text` cut into pieces of at most `max` characters, in order. A text that fits stays whole.
/// Otherwise it is cut after sentence ends, then after commas, then at `max` characters, and
/// neighbouring parts are packed together while they fit. A cut never falls inside a run of
/// Latin letters or digits unless the run alone fills a piece.
pub fn pieces(text: &str, max: usize) -> Vec<String> {
    let mut found = Vec::new();
    split(text.trim(), max.max(1), 0, &mut found);
    found
}

fn split(text: &str, max: usize, level: usize, found: &mut Vec<String>) {
    if text.chars().count() <= max {
        keep(text, found);
        return;
    }
    let ends: &[char] = match level {
        0 => &SENTENCE_ENDS,
        1 => &CLAUSE_ENDS,
        _ => return hard_cut(text, max, found),
    };
    let mut packed = String::new();
    for part in parts(text, ends) {
        let length = part.chars().count();
        if !packed.is_empty() && packed.chars().count() + length > max {
            keep(&packed, found);
            packed.clear();
        }
        if length > max {
            split(part, max, level + 1, found);
        } else {
            packed.push_str(part);
        }
    }
    keep(&packed, found);
}

/// `text` cut after every run of `ends` and the closing quotes and brackets that follow.
fn parts<'a>(text: &'a str, ends: &[char]) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if !ends.contains(&c) {
            continue;
        }
        let mut end = at + c.len_utf8();
        while let Some(&(next, d)) = chars.peek() {
            if !ends.contains(&d) && !CLOSERS.contains(&d) {
                break;
            }
            end = next + d.len_utf8();
            chars.next();
        }
        found.push(&text[start..end]);
        start = end;
    }
    if start < text.len() {
        found.push(&text[start..]);
    }
    found
}

fn hard_cut(text: &str, max: usize, found: &mut Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + max).min(chars.len());
        if end < chars.len() {
            let mut back = end;
            while back > start
                && chars[back].is_ascii_alphanumeric()
                && chars[back - 1].is_ascii_alphanumeric()
            {
                back -= 1;
            }
            if back > start {
                end = back;
            }
        }
        keep(&chars[start..end].iter().collect::<String>(), found);
        start = end;
    }
}

fn keep(piece: &str, found: &mut Vec<String>) {
    let piece = piece.trim();
    if !piece.is_empty() {
        found.push(piece.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_cut_at_sentences_then_commas_then_the_limit() {
        assert_eq!(pieces("你好。再见！", 120), ["你好。再见！"]);
        assert_eq!(
            pieces("一二三。四五六。七八九。", 8),
            ["一二三。四五六。", "七八九。"]
        );
        assert_eq!(
            pieces("一二三，四五六，七八九", 8),
            ["一二三，四五六，", "七八九"]
        );
        assert_eq!(
            pieces("一二三四五六七八九", 4),
            ["一二三四", "五六七八", "九"]
        );
        assert_eq!(
            pieces("他说：“好。”然后走了。", 8),
            ["他说：“好。”", "然后走了。"]
        );
    }

    #[test]
    fn pieces_never_split_a_latin_word_or_number() {
        assert_eq!(pieces("版本v2410已发布", 5), ["版本", "v2410", "已发布"]);
        assert_eq!(pieces("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }
}
