use lt_core::{
    bus::EventBus,
    clock::{Clock, ManualClock},
    config::Config,
    events::{EngineState, FailReason, JoinKind, PipelineEvent},
    fakes::FakeTranslator,
    queue::{
        stop_runaway, strip_wrappers, translate_one, QueuedTranscript, TranslationQueue,
        TranslatorWorker,
    },
    types::{StageTiming, StreamTime, TextClass, Transcript, UtteranceId},
};
use lt_core::{
    engines::{TranslateRequest, TranslationControl, TranslationOut, Translator, TranslatorCaps},
    error::Result,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

fn transcript(id: u64, text: &str, end: f64) -> Transcript {
    Transcript {
        id: UtteranceId(id),
        text: text.into(),
        lang_tag: Some("zh".into()),
        class: TextClass::Chinese,
        event: None,
        timing: StageTiming {
            start: StreamTime::from_seconds(end - 1.0),
            end: StreamTime::from_seconds(end),
            asr_done_ms: (end * 1000.0) as u64,
            asr_ms: 1,
        },
        absorbed: Vec::new(),
        cut: lt_core::types::CutReason::Pause,
    }
}

#[test]
fn queue_joins_waiting_items_and_preserves_leader_and_cap_order() {
    let mut cfg = Config::default().translate;
    cfg.queue_join_max_chars = 7;
    let mut queue = TranslationQueue::new(cfg);
    for id in 1..=3 {
        queue
            .push(transcript(id, "你好。", id as f64), id * 1000)
            .unwrap();
    }
    let batch = queue.take(StreamTime::from_seconds(3.0));
    let item = batch.item.unwrap();
    assert_eq!(item.transcript.id, UtteranceId(1));
    assert_eq!(item.transcript.text, "你好，你好。");
    assert_eq!(item.transcript.absorbed, vec![UtteranceId(2)]);
    assert_eq!(item.transcript.timing.end, StreamTime::from_seconds(2.0));
    assert!(matches!(
        &batch.events[0],
        PipelineEvent::Joined {
            kind: JoinKind::Queue,
            ..
        }
    ));
    assert_eq!(
        queue
            .take(StreamTime::from_seconds(3.0))
            .item
            .unwrap()
            .transcript
            .id,
        UtteranceId(3)
    );
}

#[test]
fn skip_at_more_than_six_seconds_only_and_keep_newest() {
    let mut queue = TranslationQueue::new(Config::default().translate);
    queue.push(transcript(1, "你好。", 1.0), 1000).unwrap();
    queue.push(transcript(2, "再见。", 2.0), 2000).unwrap();
    let batch = queue.take(StreamTime::from_seconds(7.5));
    assert_eq!(batch.skipped, 1);
    assert!(matches!(
        batch.events[0],
        PipelineEvent::Skipped {
            id: UtteranceId(1),
            ..
        }
    ));
    assert_eq!(batch.item.unwrap().transcript.id, UtteranceId(2));
    for live in [6.9, 7.0] {
        let mut queue = TranslationQueue::new(Config::default().translate);
        queue.push(transcript(1, "你好。", 1.0), 1000).unwrap();
        queue.push(transcript(2, "再见。", 2.0), 2000).unwrap();
        assert_eq!(queue.take(StreamTime::from_seconds(live)).skipped, 0);
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
    };
    let mut fake = FakeTranslator::default();
    for (response, expected) in [
        ("你好。", None),
        ("Translation: \"Hello.\"", Some("Hello.")),
        ("Hello. the the the the", Some("Hello.")),
        ("The 胖est.", Some("The 胖est.")),
    ] {
        fake.responses.insert("你好。".into(), response.into());
        let event = translate_one(
            &mut fake,
            &item,
            &Config::default().translate,
            &clock,
            &bus,
            Arc::new(AtomicBool::new(false)),
        );
        if let Some(expected) = expected {
            assert!(matches!(event, PipelineEvent::TranslationFinal{text,..} if text==expected));
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
        delta_delay: Duration::from_millis(3),
        ..FakeTranslator::default()
    };
    fake.responses
        .insert("你好。".into(), "Hello there.".into());
    let metrics = fake.metrics.clone();
    let mut config = Config::default().translate;
    config.timeout_s = 0.04;
    let stop = Arc::new(AtomicBool::new(false));
    let mut worker = TranslatorWorker::start(Box::new(fake), config, clock, bus, stop).unwrap();
    worker
        .input
        .send(QueuedTranscript {
            transcript: transcript(1, "超时。", 1.0),
            queued_ms: 1000,
        })
        .unwrap();
    assert!(matches!(
        worker
            .completed
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
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
        })
        .unwrap();
    assert!(matches!(
        worker
            .completed
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
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
    let start = Instant::now();
    worker.stop().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
}

struct ScriptedTranslator {
    clock: ManualClock,
    fragments: Vec<String>,
    final_text: String,
    expected_source: String,
    wait_after_stream: bool,
    reached_end: Arc<AtomicBool>,
}

impl ScriptedTranslator {
    fn new(clock: ManualClock, fragments: &[&str], final_text: &str) -> Self {
        Self {
            clock,
            fragments: fragments
                .iter()
                .map(|fragment| (*fragment).into())
                .collect(),
            final_text: final_text.into(),
            expected_source: "zh".into(),
            wait_after_stream: false,
            reached_end: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Translator for ScriptedTranslator {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            streaming: true,
            glossary: false,
            context: false,
            prefill: false,
            max_input_chars: 300,
            pairs: vec![("zh".into(), "en".into())],
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
        assert_eq!(request.src, self.expected_source);
        let mut text = String::new();
        for fragment in &self.fragments {
            request.control.check()?;
            self.clock.advance(StreamTime::from_millis(100));
            text.push_str(fragment);
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
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut config = Config::default().translate;
    config.timeout_s = 0.2;
    let item = QueuedTranscript {
        transcript: transcript(1, "你好。", 1.0),
        queued_ms: 1200,
    };
    let bus = EventBus::default();
    let deltas = bus.subscribe(8);
    let start = Instant::now();
    let event = translate_one(&mut fake, &item, &config, &clock, &bus, cancelled.clone());
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!reached_end.load(Ordering::SeqCst));
    assert!(!cancelled.load(Ordering::SeqCst));
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
    let next_event = translate_one(&mut next, &item, &config, &clock, &bus, cancelled);
    assert!(
        matches!(next_event, PipelineEvent::TranslationFinal { text, .. } if text == "Goodbye.")
    );
}
