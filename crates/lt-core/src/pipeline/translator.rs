//! The translator thread: one request at a time, one terminal event per request.

use super::{cancel::Cancel, queue::QueuedTranscript};
use crate::{
    bus::EventBus,
    clock::Clock,
    engines::{TranslateRequest, TranslationControl, Translator},
    error::{Error, Result},
    events::{EngineKind, EngineState, FailReason, PipelineEvent, Timing},
    text::chinese_chars,
    types::UtteranceId,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Return the prefix before a consecutive threefold repeat of a 1–3 word n-gram.
fn stop_runaway(text: &str) -> Option<String> {
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

fn strip_wrappers(text: &str) -> String {
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

fn translate_one(
    translator: &mut dyn Translator,
    item: &QueuedTranscript,
    clock: &dyn Clock,
    bus: &EventBus,
    cancelled: Cancel,
) -> PipelineEvent {
    let source = &item.transcript;
    let span = tracing::info_span!("translation", id = source.id.0);
    let _utterance = span.enter();
    let sent_ms = clock.now().millis();
    let control = TranslationControl {
        deadline: Instant::now() + crate::engines::REQUEST_TIMEOUT,
        cancelled: cancelled.flag(),
        abort: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let text = source.text.as_str();
    let request = TranslateRequest {
        id: source.id,
        text,
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
            control
                .abort
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let visible = stopped_at.as_ref().unwrap_or(&cleaned);
        if !visible.is_empty() && item.preview.is_none() {
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
    // Keep the prefix it reached instead of turning the result into a timeout.
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

/// The event a finished preview publishes, if any: an answer with Chinese in it explains the
/// text instead of translating it, which a preview of a few characters makes the model do.
fn preview_event(event: PipelineEvent, ids: Vec<UtteranceId>) -> Option<PipelineEvent> {
    match event {
        PipelineEvent::TranslationFinal { id, text, .. } if chinese_chars(&text) == 0 => {
            Some(PipelineEvent::TranslationPreview { id, ids, text })
        }
        _ => None,
    }
}

/// The scheduler publishes each completion before submitting another item, so line terminals
/// cannot be reordered. A preview completes with its event, or with `None` when it shows nothing.
pub(super) struct TranslatorWorker {
    pub(super) input: Sender<QueuedTranscript>,
    pub(super) completed: Receiver<Option<PipelineEvent>>,
    join: Option<JoinHandle<()>>,
    cancelled: Cancel,
}

impl TranslatorWorker {
    pub(super) fn start(
        mut translator: Box<dyn Translator>,
        clock: Arc<dyn Clock>,
        bus: EventBus,
        cancelled: Cancel,
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
                    deadline: Instant::now() + crate::engines::REQUEST_TIMEOUT,
                    cancelled: worker_cancelled.flag(),
                    abort: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
                while !worker_cancelled.is_set() {
                    let item = match work.recv_timeout(Duration::from_millis(20)) {
                        Ok(item) => item,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
                    let event = translate_one(
                        translator.as_mut(),
                        &item,
                        clock.as_ref(),
                        &bus,
                        worker_cancelled.clone(),
                    );
                    let event = match item.preview {
                        Some(ids) => preview_event(event, ids),
                        None => Some(event),
                    };
                    // Cooperative channel send: shutdown must not wait for scheduler.
                    let mut pending = event;
                    loop {
                        if worker_cancelled.is_set() {
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

    pub(super) fn stop(&mut self) -> Result<()> {
        self.cancelled.set();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bus::EventBus,
        clock::ManualClock,
        engines::{TranslationOut, TranslatorCaps},
        events::EngineState,
        fakes::FakeTranslator,
        types::{CutReason, StageTiming, StreamTime, TextClass, Transcript},
    };
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };

    /// Only stops a hung test; no assertion depends on how fast this PC is.
    const WATCHDOG: Duration = Duration::from_secs(30);

    fn transcript(id: u64, text: &str, end: f64) -> Transcript {
        Transcript {
            id: UtteranceId(id),
            text: text.into(),
            class: TextClass::Chinese,
            timing: StageTiming {
                start: StreamTime::from_seconds(end - 1.0),
                end: StreamTime::from_seconds(end),
                asr_done_ms: (end * 1000.0) as u64,
                asr_ms: 1,
            },
            absorbed: Vec::new(),
            cut: CutReason::Pause,
            logprob: None,
            second_pass: crate::types::SecondPass::default(),
        }
    }

    #[test]
    fn wrappers_runaway_echo_and_leaked_chinese_are_checked() {
        assert_eq!(strip_wrappers(" Translation: \"Hello.\" \n"), "Hello.");
        assert_eq!(
            stop_runaway("Hello. the the the the"),
            Some("Hello.".into())
        );
        assert_eq!(
            stop_runaway("Hello. go back go back go back"),
            Some("Hello.".into())
        );
        assert_eq!(
            stop_runaway("Hello. a b c a b c a b c"),
            Some("Hello.".into())
        );
        assert!(stop_runaway("We can look at this again.").is_none());
        let bus = EventBus::default();
        let clock = ManualClock::default();
        clock.set(StreamTime::from_seconds(5.0));
        let item = QueuedTranscript {
            transcript: transcript(1, "你好。", 1.0),
            queued_ms: 1500,
            preview: None,
        };
        let mut fake = FakeTranslator::default();
        for (response, expected) in [
            ("你好。", None),
            ("Translation: \"Hello.\"", Some("Hello.")),
            ("Hello. the the the the", Some("Hello.")),
            ("The 胖est.", Some("The 胖est.")),
        ] {
            fake.responses.insert("你好。".into(), response.into());
            let event = translate_one(&mut fake, &item, &clock, &bus, Cancel::default());
            if let Some(expected) = expected {
                assert!(
                    matches!(event, PipelineEvent::TranslationFinal{text,..} if text==expected)
                );
            } else {
                assert!(matches!(
                    event,
                    PipelineEvent::TranslationFailed {
                        reason: FailReason::Echo,
                        ..
                    }
                ));
            }
        }
    }

    #[test]
    fn worker_warms_up_then_serializes_requests_and_recovers_from_timeout() {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::default());
        let bus = EventBus::default();
        let events = bus.subscribe(100);
        let mut fake = FakeTranslator {
            timeout_text: Some("超时。".into()),
            ..FakeTranslator::default()
        };
        fake.responses
            .insert("你好。".into(), "Hello there.".into());
        let metrics = fake.metrics.clone();
        let mut worker =
            TranslatorWorker::start(Box::new(fake), clock, bus, Cancel::default()).unwrap();
        worker
            .input
            .send(QueuedTranscript {
                transcript: transcript(1, "超时。", 1.0),
                queued_ms: 1000,
                preview: None,
            })
            .unwrap();
        assert!(matches!(
            worker.completed.recv_timeout(WATCHDOG).unwrap().unwrap(),
            PipelineEvent::TranslationFailed {
                reason: FailReason::Timeout,
                ..
            }
        ));
        worker
            .input
            .send(QueuedTranscript {
                transcript: transcript(2, "你好。", 2.0),
                queued_ms: 2000,
                preview: None,
            })
            .unwrap();
        assert!(matches!(
            worker.completed.recv_timeout(WATCHDOG).unwrap().unwrap(),
            PipelineEvent::TranslationFinal { .. }
        ));
        assert_eq!(metrics.warmups.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.requests.load(Ordering::SeqCst), 2);
        assert_eq!(metrics.max_active.load(Ordering::SeqCst), 1);
        let recorded: Vec<_> = events.try_iter().collect();
        let ready = recorded
            .iter()
            .position(|event| {
                matches!(
                    event,
                    PipelineEvent::EngineStatus {
                        state: EngineState::Ready,
                        ..
                    }
                )
            })
            .unwrap();
        let delta = recorded
            .iter()
            .position(|event| matches!(event, PipelineEvent::TranslationDelta { .. }))
            .unwrap();
        assert!(ready < delta);
        worker.stop().unwrap();
    }

    struct ScriptedTranslator {
        clock: ManualClock,
        chunks: Vec<String>,
        final_text: String,
        wait_after_stream: bool,
        reached_end: Arc<AtomicBool>,
    }

    impl ScriptedTranslator {
        fn new(clock: ManualClock, chunks: &[&str], final_text: &str) -> Self {
            Self {
                clock,
                chunks: chunks.iter().map(|chunk| (*chunk).into()).collect(),
                final_text: final_text.into(),
                wait_after_stream: false,
                reached_end: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl Translator for ScriptedTranslator {
        fn caps(&self) -> TranslatorCaps {
            TranslatorCaps {
                max_input_chars: 300,
            }
        }

        fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
            control.check()
        }

        fn translate(
            &mut self,
            request: &TranslateRequest<'_>,
            on_delta: &mut dyn FnMut(&str),
        ) -> Result<TranslationOut> {
            let mut text = String::new();
            for chunk in &self.chunks {
                request.control.check()?;
                self.clock.advance(StreamTime::from_millis(100));
                text.push_str(chunk);
                on_delta(&text);
                request.control.check()?;
            }
            if self.wait_after_stream {
                loop {
                    request.control.check()?;
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            self.reached_end.store(true, Ordering::SeqCst);
            self.clock.advance(StreamTime::from_millis(100));
            Ok(TranslationOut {
                text: self.final_text.clone(),
                prompt_tokens: 17,
                cached_tokens: 31,
                generated_tokens: 9,
                truncated: false,
            })
        }
    }

    #[test]
    fn runaway_aborts_the_request_early_and_retains_its_prefix_without_stopping_the_worker() {
        let clock = ManualClock::new(StreamTime::from_millis(1400));
        let mut fake = ScriptedTranslator::new(
            clock.clone(),
            &[
                "Translation: \"Hello. go back go back go back ",
                "Never show this.",
            ],
            "Never return this.",
        );
        fake.wait_after_stream = true;
        let reached_end = fake.reached_end.clone();
        let cancelled = Cancel::default();
        let item = QueuedTranscript {
            transcript: transcript(1, "你好。", 1.0),
            queued_ms: 1200,
            preview: None,
        };
        let bus = EventBus::default();
        let deltas = bus.subscribe(8);
        let event = translate_one(&mut fake, &item, &clock, &bus, cancelled.clone());
        assert!(!reached_end.load(Ordering::SeqCst));
        assert!(!cancelled.is_set());
        assert_eq!(clock.now(), StreamTime::from_millis(1500));
        assert!(matches!(event, PipelineEvent::TranslationFinal { text, .. } if text == "Hello."));
        let recorded: Vec<_> = deltas.try_iter().collect();
        assert_eq!(recorded.len(), 1);
        assert!(
            matches!(&recorded[0], PipelineEvent::TranslationDelta { text_so_far, .. } if text_so_far == "Hello.")
        );
        // An abort belongs to one request; the same pipeline cancellation flag can
        // be reused for the next request without suppressing it.
        let mut next = ScriptedTranslator::new(clock.clone(), &["Goodbye."], "Goodbye.");
        let next_event = translate_one(&mut next, &item, &clock, &bus, cancelled);
        assert!(
            matches!(next_event, PipelineEvent::TranslationFinal { text, .. } if text == "Goodbye.")
        );
    }
}
