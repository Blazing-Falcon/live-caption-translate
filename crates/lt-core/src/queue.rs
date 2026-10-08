//! Queue policy is independent of engine execution so live audio and hold
//! deadlines continue advancing while exactly one translation is in flight.
use crate::{
    bus::EventBus,
    clock::Clock,
    config::TranslateConfig,
    engines::{TranslateRequest, TranslationControl, Translator},
    error::{Error, Result},
    events::{EngineKind, EngineState, FailReason, JoinKind, PipelineEvent, SkipReason, Timing},
    join::join_text,
    text::chinese_chars,
    types::{StreamTime, Transcript},
};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct QueuedTranscript {
    pub transcript: Transcript,
    pub queued_ms: u64,
}

#[derive(Default)]
pub struct QueueBatch {
    pub item: Option<QueuedTranscript>,
    pub events: Vec<PipelineEvent>,
    pub skipped: u32,
}

pub struct TranslationQueue {
    config: TranslateConfig,
    waiting: VecDeque<QueuedTranscript>,
}

impl TranslationQueue {
    pub const CAPACITY: usize = 256;
    pub fn new(config: TranslateConfig) -> Self {
        Self {
            config,
            waiting: VecDeque::new(),
        }
    }
    pub fn push(&mut self, transcript: Transcript, queued_ms: u64) -> Result<()> {
        if self.waiting.len() == Self::CAPACITY {
            return Err(Error::Engine("Translation queue is full".into()));
        }
        self.waiting.push_back(QueuedTranscript {
            transcript,
            queued_ms,
        });
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.waiting.len()
    }
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
    pub fn oldest_end(&self) -> Option<StreamTime> {
        self.waiting.front().map(|item| item.transcript.timing.end)
    }

    pub fn take(&mut self, live: StreamTime) -> QueueBatch {
        let mut batch = QueueBatch::default();
        let skip = self.waiting.front().is_some_and(|item| {
            live.saturating_sub(item.transcript.timing.end).seconds()
                > f64::from(self.config.skip_lag_s)
        });
        if skip {
            while self.waiting.len() > 1 {
                if let Some(old) = self.waiting.pop_front() {
                    batch.events.push(PipelineEvent::Skipped {
                        id: old.transcript.id,
                        reason: SkipReason::CatchUp,
                    });
                    batch.skipped += 1;
                }
            }
        }
        let Some(mut item) = self.waiting.pop_front() else {
            return batch;
        };
        let mut absorbed = Vec::new();
        while let Some(next) = self.waiting.front() {
            let joined = join_text(&item.transcript.text, &next.transcript.text);
            if joined.chars().count() > self.config.queue_join_max_chars as usize {
                break;
            }
            let Some(next) = self.waiting.pop_front() else {
                break;
            };
            absorbed.push(next.transcript.id);
            item.transcript.absorbed.push(next.transcript.id);
            item.transcript
                .absorbed
                .extend(next.transcript.absorbed.iter().copied());
            item.transcript.text = joined;
            item.transcript.timing.end = next.transcript.timing.end;
            item.transcript.timing.asr_done_ms = item
                .transcript
                .timing
                .asr_done_ms
                .max(next.transcript.timing.asr_done_ms);
            item.transcript.timing.asr_ms = item
                .transcript
                .timing
                .asr_ms
                .saturating_add(next.transcript.timing.asr_ms);
            if next.transcript.class == crate::types::TextClass::Mixed {
                item.transcript.class = crate::types::TextClass::Mixed;
            }
        }
        if !absorbed.is_empty() {
            batch.events.push(PipelineEvent::Joined {
                id: item.transcript.id,
                absorbed,
                text: item.transcript.text.clone(),
                kind: JoinKind::Queue,
            });
        }
        batch.item = Some(item);
        batch
    }
}

pub fn strip_wrappers(text: &str) -> String {
    let text = text.trim();
    let text = text.strip_prefix("Translation:").unwrap_or(text).trim();
    let text = if text.len() >= 2
        && ((text.starts_with('"') && text.ends_with('"'))
            || (text.starts_with('\'') && text.ends_with('\'')))
    {
        &text[1..text.len() - 1]
    } else {
        text
    };
    text.trim().to_owned()
}

/// Keep trailing whitespace for the repetition guard, but do not expose a
/// possible wrapper prefix or an opening quote while the response streams.
fn strip_streaming_wrappers(text: &str) -> &str {
    let text = text.trim_start();
    if "Translation:".starts_with(text) {
        return "";
    }
    let text = text
        .strip_prefix("Translation:")
        .unwrap_or(text)
        .trim_start();
    if let Some(opening) = text
        .chars()
        .next()
        .filter(|character| matches!(character, '"' | '\''))
    {
        let text = &text[opening.len_utf8()..];
        // The closing quote may arrive with the final token. Removing it here
        // keeps deltas and the final consistent without requiring it to arrive
        // before showing the words inside the quote.
        let trimmed = text.trim_end();
        trimmed.strip_suffix(opening).unwrap_or(text).trim_start()
    } else {
        text
    }
}

fn source_language(tag: Option<&str>) -> String {
    let tag = tag.unwrap_or("zh").trim();
    let tag = tag
        .strip_prefix("<|")
        .and_then(|tag| tag.strip_suffix("|>"))
        .unwrap_or(tag);
    if tag.is_empty() {
        "zh".into()
    } else {
        tag.to_ascii_lowercase()
    }
}

/// Return the prefix before a consecutive threefold repeat of a 1–3 word n-gram.
pub fn stop_runaway(text: &str) -> Option<String> {
    let mut words = Vec::new();
    let mut in_word = false;
    for (offset, c) in text.char_indices() {
        if c.is_whitespace() {
            in_word = false;
        } else if !in_word {
            words.push(offset);
            in_word = true;
        }
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    for start in 0..tokens.len() {
        for size in 1..=3 {
            if start + size * 3 > tokens.len() {
                continue;
            }
            let group = &tokens[start..start + size];
            if group == &tokens[start + size..start + size * 2]
                && group == &tokens[start + size * 2..start + size * 3]
            {
                return Some(text[..words[start]].trim_end().into());
            }
        }
    }
    None
}

fn stop_runaway_streaming(text: &str) -> Option<String> {
    // The current token may still be a prefix (a -> about). Only compare words
    // whose trailing whitespace has arrived, except when checking final output.
    let boundary = text
        .char_indices()
        .rev()
        .find_map(|(offset, c)| c.is_whitespace().then_some(offset))?;
    stop_runaway(&text[..boundary])
}

pub fn translate_one(
    translator: &mut dyn Translator,
    item: &QueuedTranscript,
    config: &TranslateConfig,
    clock: &dyn Clock,
    bus: &EventBus,
    cancelled: Arc<AtomicBool>,
) -> PipelineEvent {
    let source = &item.transcript;
    let span = tracing::info_span!("translation", id = source.id.0);
    let _utterance = span.enter();
    let sent_ms = clock.now().millis();
    let control = TranslationControl {
        deadline: Instant::now() + Duration::from_secs_f32(config.timeout_s),
        cancelled,
        abort: Arc::new(AtomicBool::new(false)),
    };
    let source_language = source_language(source.lang_tag.as_deref());
    let request = TranslateRequest {
        id: source.id,
        text: &source.text,
        src: &source_language,
        tgt: &config.target,
        terms: &[],
        context: &[],
        prefill: "",
        max_tokens: None,
        control: control.clone(),
    };
    let mut first_token_ms = None;
    let mut stopped_at = None;
    let mut on_delta = |text: &str| {
        if control.check().is_err() || stopped_at.is_some() {
            return;
        }
        let content = strip_streaming_wrappers(text);
        let cleaned = content.trim_end().to_owned();
        if let Some(prefix) = stop_runaway_streaming(content) {
            stopped_at = Some(prefix);
            control.abort.store(true, Ordering::Relaxed);
        }
        let visible = stopped_at.as_ref().unwrap_or(&cleaned);
        if !visible.is_empty() {
            first_token_ms.get_or_insert_with(|| clock.now().millis());
            bus.publish(PipelineEvent::TranslationDelta {
                id: source.id,
                text_so_far: visible.clone(),
            });
        }
    };
    let result = if source.text.chars().count() > translator.caps().max_input_chars {
        Err(Error::Translation {
            reason: FailReason::Error,
            message: "This line is too long for the translator".into(),
        })
    } else {
        control
            .check()
            .and_then(|()| translator.translate(&request, &mut on_delta))
            .and_then(|output| {
                control.check()?;
                Ok(output)
            })
    };
    // A cooperative engine may return as soon as the runaway guard fires.
    // Retain the measured prefix rather than converting it into a timeout.
    let result = match result {
        Err(Error::Translation {
            reason: FailReason::Runaway,
            ..
        }) if stopped_at.is_some() => Ok(crate::engines::TranslationOut {
            text: stopped_at.clone().unwrap_or_default(),
            ..crate::engines::TranslationOut::default()
        }),
        other => other,
    };
    match result {
        Ok(output) => {
            let cleaned = strip_wrappers(&output.text);
            let repeated = stopped_at.or_else(|| stop_runaway(&cleaned));
            let empty_reason = if repeated.is_some() {
                FailReason::Runaway
            } else {
                FailReason::Error
            };
            let text = repeated.unwrap_or(cleaned);
            if text.is_empty() {
                return PipelineEvent::TranslationFailed {
                    id: source.id,
                    reason: empty_reason,
                    message: "Could not translate this line".into(),
                };
            }
            if text == source.text
                && matches!(
                    source.class,
                    crate::types::TextClass::Chinese | crate::types::TextClass::Mixed
                )
            {
                return PipelineEvent::TranslationFailed {
                    id: source.id,
                    reason: FailReason::Echo,
                    message: "Could not translate this line".into(),
                };
            }
            let leaked = chinese_chars(&text);
            if leaked > 0 {
                tracing::warn!(
                    id = source.id.0,
                    leaked_cjk = leaked,
                    "Chinese characters in translated output"
                );
            }
            PipelineEvent::TranslationFinal {
                id: source.id,
                text,
                timing: Timing {
                    speech_end_ms: source.timing.end.millis(),
                    asr_done_ms: source.timing.asr_done_ms,
                    queued_ms: item.queued_ms,
                    sent_ms,
                    first_token_ms,
                    done_ms: clock.now().millis(),
                    prompt_tokens: output.prompt_tokens,
                    cached_tokens: output.cached_tokens,
                    generated_tokens: output.generated_tokens,
                },
            }
        }
        Err(error) => {
            let reason = match &error {
                Error::Translation { reason, .. } => *reason,
                Error::Stopped => FailReason::Error,
                _ => FailReason::Error,
            };
            PipelineEvent::TranslationFailed {
                id: source.id,
                reason,
                message: error.to_string(),
            }
        }
    }
}

/// Completion notifications carry the terminal event; the scheduler publishes
/// them before submitting another item so line terminals cannot be reordered.
pub struct TranslatorWorker {
    pub input: Sender<QueuedTranscript>,
    pub completed: Receiver<PipelineEvent>,
    join: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
}

impl TranslatorWorker {
    pub fn start(
        mut translator: Box<dyn Translator>,
        config: TranslateConfig,
        clock: Arc<dyn Clock>,
        bus: EventBus,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (input, work): (Sender<QueuedTranscript>, Receiver<QueuedTranscript>) = bounded(1);
        let (complete, completed) = bounded(1);
        let worker_cancelled = cancelled.clone();
        let join = thread::Builder::new()
            .name("lt-translator".into())
            .spawn(move || {
                bus.publish(PipelineEvent::EngineStatus {
                    engine: EngineKind::Translator,
                    state: EngineState::Loading,
                    message: None,
                });
                let control = TranslationControl {
                    deadline: Instant::now() + Duration::from_secs_f32(config.timeout_s),
                    cancelled: worker_cancelled.clone(),
                    abort: Arc::new(AtomicBool::new(false)),
                };
                let warmup = translator.warm_up(&control);
                bus.publish(PipelineEvent::EngineStatus {
                    engine: EngineKind::Translator,
                    state: if warmup.is_ok() {
                        EngineState::Ready
                    } else {
                        EngineState::Failed
                    },
                    message: warmup.err().map(|error| error.to_string()),
                });
                while !worker_cancelled.load(Ordering::Relaxed) {
                    let item = match work.recv_timeout(Duration::from_millis(20)) {
                        Ok(item) => item,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
                    let event = translate_one(
                        translator.as_mut(),
                        &item,
                        &config,
                        clock.as_ref(),
                        &bus,
                        worker_cancelled.clone(),
                    );
                    // Cooperative channel send: shutdown must not wait for scheduler.
                    let mut pending = event;
                    loop {
                        if worker_cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        match complete.send_timeout(pending, Duration::from_millis(20)) {
                            Ok(()) => break,
                            Err(crossbeam_channel::SendTimeoutError::Timeout(event)) => {
                                pending = event
                            }
                            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => return,
                        }
                    }
                }
            })?;
        Ok(Self {
            input,
            completed,
            join: Some(join),
            cancelled,
        })
    }

    pub fn stop(&mut self) -> Result<()> {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| Error::Engine("Translator worker stopped unexpectedly".into()))?;
        }
        Ok(())
    }
}

impl Drop for TranslatorWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
