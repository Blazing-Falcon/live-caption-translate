//! One screen job: read the text, group it, translate each block.

use super::{
    filters, group, ink,
    line::{Line, RawLine},
    Direction, ImageView, ScreenEvent,
};
use crate::{
    engines::{TextRecognizer, TranslationControl, Translator, TranslatorCaps, REQUEST_TIMEOUT},
    error::Result,
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
/// The cache is emptied when it holds this many entries, which bounds its memory.
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

/// Reads `image`, groups its text into blocks and translates batches one request at a time. The
/// events follow the order described on [`ScreenEvent`]. Once `cancel` is set the job sends no
/// further request and emits nothing more. A block whose piece fails is reported as failed and
/// the next block still runs.
pub fn run_job(
    job: u64,
    image: &ImageView<'_>,
    recognizer: &mut dyn TextRecognizer,
    translator: &mut dyn FnMut(&[&str]) -> Result<Vec<String>>,
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
            direction: Direction::Horizontal,
            translator_caps: TranslatorCaps::default(),
        },
    );
}

pub type PreparePaint<'a> = dyn FnMut(&ImageView<'_>, &[Line], &[super::Block]) -> Result<()> + 'a;
pub struct JobOutput<'a> {
    pub translator_caps: TranslatorCaps,
    pub direction: Direction,
    pub debug: bool,
    pub emit: &'a mut dyn FnMut(ScreenEvent),
    pub prepare: Option<&'a mut PreparePaint<'a>>,
}

pub fn run_job_with_output(
    job: u64,
    image: &ImageView<'_>,
    recognizer: &mut dyn TextRecognizer,
    translator: &mut dyn FnMut(&[&str]) -> Result<Vec<String>>,
    cancel: &AtomicBool,
    cache: &mut TranslationCache,
    output: JobOutput<'_>,
) {
    let JobOutput {
        emit,
        prepare,
        debug,
        direction,
        translator_caps,
    } = output;
    let started = Instant::now();
    let cancelled = || cancel.load(Ordering::Acquire);
    if cancelled() {
        return;
    }
    emit(ScreenEvent::Reading { job });
    let lines = match recognizer.recognize_direction(image, direction) {
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
    let groups = match direction {
        Direction::Horizontal => {
            let mut facts = filters::prepare(&raw);
            ink::annotate(image, &mut facts);
            group::group_screenshot(&facts)
        }
        Direction::Vertical => {
            let facts = super::vertical::prepare_lines(&lines);
            let groups = super::vertical::grouping::group(facts, Some(image), Some(true));
            super::tilt::level(&groups).into_owned()
        }
    };
    let blocks = group::blocks_from_groups_direction(image, &groups, direction);
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
    translate_blocks(
        job,
        sources,
        translator,
        &translator_caps,
        cancel,
        cache,
        emit,
    );
    if cancelled() {
        return;
    }
    emit(ScreenEvent::Done {
        job,
        total_ms: millis(started),
    });
}

struct PendingBlock {
    id: u32,
    pieces: Vec<String>,
    answers: Vec<Option<String>>,
    failed: Option<String>,
}

fn translate_blocks(
    job: u64,
    sources: Vec<(u32, String)>,
    translator: &mut dyn FnMut(&[&str]) -> Result<Vec<String>>,
    caps: &TranslatorCaps,
    cancel: &AtomicBool,
    cache: &mut TranslationCache,
    emit: &mut dyn FnMut(ScreenEvent),
) {
    let mut blocks: Vec<_> = sources
        .into_iter()
        .map(|(id, source)| {
            let pieces = pieces(&source, PIECE_CHARS)
                .into_iter()
                .flat_map(|piece| {
                    if piece.encode_utf16().count() > caps.max_batch_chars {
                        pieces(&piece, (caps.max_batch_chars / 2).max(1))
                    } else {
                        vec![piece]
                    }
                })
                .collect::<Vec<_>>();
            PendingBlock {
                id,
                answers: vec![None; pieces.len()],
                pieces,
                failed: None,
            }
        })
        .collect();
    let positions: Vec<_> = blocks
        .iter()
        .enumerate()
        .flat_map(|(block, pending)| (0..pending.pieces.len()).map(move |piece| (block, piece)))
        .collect();
    let mut cursor = 0;
    let mut reported = 0;
    while reported < blocks.len() {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let mut batch: Vec<String> = Vec::new();
        let mut owners: Vec<Vec<(usize, usize)>> = Vec::new();
        let mut characters = 0;
        while let Some(&(block, piece)) = positions.get(cursor) {
            if batch.len() >= caps.max_batch_items.max(1) {
                break;
            }
            if blocks[block].failed.is_some() {
                cursor += 1;
                continue;
            }
            let source = &blocks[block].pieces[piece];
            if let Some(known) = cache.entries.get(source) {
                blocks[block].answers[piece] = Some(known.clone());
                if batch.is_empty() {
                    report_ready(job, &blocks, &mut reported, cancel, emit);
                    if cancel.load(Ordering::Acquire) {
                        return;
                    }
                }
                cursor += 1;
                continue;
            }
            if let Some(at) = batch.iter().position(|text| text == source) {
                owners[at].push((block, piece));
                cursor += 1;
                continue;
            }
            let length = source.encode_utf16().count();
            if !batch.is_empty() && characters + length > caps.max_batch_chars {
                break;
            }
            characters += length;
            batch.push(source.clone());
            owners.push(vec![(block, piece)]);
            cursor += 1;
        }
        if !batch.is_empty() {
            let inputs: Vec<_> = batch.iter().map(String::as_str).collect();
            let result = translator(&inputs);
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let answers = match result {
                Ok(answers) if answers.len() == inputs.len() => Ok(answers),
                Ok(_) => Err("Translation answer count does not match request".to_owned()),
                Err(error) => Err(error.to_string()),
            };
            match answers {
                Ok(answers) => {
                    for ((source, owners), text) in batch.into_iter().zip(owners).zip(answers) {
                        let text = text.trim().to_owned();
                        if !text.is_empty() {
                            cache.insert(source, text.clone());
                        }
                        for (block, piece) in owners {
                            if text.is_empty() {
                                blocks[block].failed = Some("The translation is empty".into());
                            } else {
                                blocks[block].answers[piece] = Some(text.clone());
                            }
                        }
                    }
                }
                Err(message) => {
                    for (block, _) in owners.into_iter().flatten() {
                        blocks[block].failed = Some(message.clone());
                    }
                }
            }
        }
        report_ready(job, &blocks, &mut reported, cancel, emit);
    }
}

fn report_ready(
    job: u64,
    blocks: &[PendingBlock],
    reported: &mut usize,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(ScreenEvent),
) {
    while let Some(block) = blocks.get(*reported) {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let event = if let Some(message) = &block.failed {
            ScreenEvent::Failed {
                job,
                id: block.id,
                message: message.clone(),
            }
        } else if block.answers.iter().all(Option::is_some) {
            ScreenEvent::Translated {
                job,
                id: block.id,
                text: block
                    .answers
                    .iter()
                    .map(|answer| answer.as_deref().unwrap())
                    .collect::<Vec<_>>()
                    .join(" "),
            }
        } else {
            break;
        };
        emit(event);
        *reported += 1;
    }
}

fn millis(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// Adapts a [`Translator`] to screen batches, each with its own deadline and cancellation flag.
pub fn batch_translator<'a>(
    translator: &'a mut dyn Translator,
    cancelled: &Arc<AtomicBool>,
) -> impl FnMut(&[&str]) -> Result<Vec<String>> + 'a {
    let cancelled = cancelled.clone();
    move |texts| {
        let control = TranslationControl {
            deadline: Instant::now() + REQUEST_TIMEOUT,
            cancelled: cancelled.clone(),
            abort: Arc::new(AtomicBool::new(false)),
        };
        Ok(translator
            .translate_batch(texts, &control)?
            .into_iter()
            .map(|answer| answer.text)
            .collect())
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
