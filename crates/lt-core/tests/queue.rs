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
    error::{Error, Result},
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
fn queue_joins_three_items_in_one_request_and_does_not_reannounce_terminal_ids() {
    let mut queue = TranslationQueue::new(Config::default().translate);
    queue.push(transcript(1, "第一句话。", 1.0), 1000).unwrap();
    let mut next = transcript(2, "第二句话。", 2.0);
    next.absorbed = vec![UtteranceId(3)];
    queue.push(next, 2000).unwrap();
    queue.push(transcript(4, "第三句话。", 3.0), 3000).unwrap();
    let batch = queue.take(StreamTime::from_seconds(3.0));
    assert!(
        matches!(&batch.events[0],PipelineEvent::Joined{absorbed,..} if absorbed == &vec![UtteranceId(2),UtteranceId(4)])
    );
    let mut ids = batch.item.unwrap().transcript.absorbed;
    ids.sort();
    assert_eq!(ids, vec![UtteranceId(2), UtteranceId(3), UtteranceId(4)]);
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

#[test]
fn cancellation_joins_running_translator_worker_without_leaking_a_thread() {
    let fake = FakeTranslator {
        delay_per_character: Duration::from_secs(1),
        ..FakeTranslator::default()
    };
    let metrics = fake.metrics.clone();
    let mut worker = TranslatorWorker::start(
        Box::new(fake),
        Config::default().translate,
        Arc::new(ManualClock::default()),
        EventBus::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    worker
        .input
        .send(QueuedTranscript {
            transcript: transcript(1, "你好。", 1.0),
            queued_ms: 1000,
        })
        .unwrap();
    let wait = Instant::now();
    while metrics.active.load(Ordering::SeqCst) == 0 && wait.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(metrics.active.load(Ordering::SeqCst), 1);
    let start = Instant::now();
    worker.stop().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(metrics.active.load(Ordering::SeqCst), 0);
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
fn fragmented_wrappers_do_not_leak_or_count_as_the_first_translated_word() {
    let cases: &[(&[&str], &str, &[&str], u64)] = &[
        (
            &[
                "Tran", "sl", "ation", ":", " ", "\"", "Hel", "lo", ".", "\"",
            ],
            "Translation: \"Hello.\"",
            &["Hel", "Hello", "Hello.", "Hello."],
            2100,
        ),
        (
            &["\"", "Hel", "lo.", "\""],
            "\"Hello.\"",
            &["Hel", "Hello.", "Hello."],
            1600,
        ),
        (
            &["'", "Hello", ".", "'"],
            "'Hello.'",
            &["Hello", "Hello.", "Hello."],
            1600,
        ),
        // A possible wrapper prefix that becomes ordinary speech is released
        // once it can no longer be the "Translation:" label.
        (
            &["Tran", "sit is ready."],
            "Transit is ready.",
            &["Transit is ready."],
            1600,
        ),
    ];
    for (fragments, response, expected_deltas, expected_first) in cases {
        let clock = ManualClock::new(StreamTime::from_millis(1400));
        let bus = EventBus::default();
        let deltas = bus.subscribe(32);
        let mut fake = ScriptedTranslator::new(clock.clone(), fragments, response);
        let mut source = transcript(1, "你好。", 1.0);
        source.timing.asr_done_ms = 1100;
        source.lang_tag = Some("<|ZH|>".into());
        let item = QueuedTranscript {
            transcript: source,
            queued_ms: 1200,
        };
        let event = translate_one(
            &mut fake,
            &item,
            &Config::default().translate,
            &clock,
            &bus,
            Arc::new(AtomicBool::new(false)),
        );
        let actual_deltas: Vec<String> = deltas
            .try_iter()
            .map(|event| match event {
                PipelineEvent::TranslationDelta {
                    id: UtteranceId(1),
                    text_so_far,
                } => text_so_far,
                other => panic!("Unexpected streaming event: {other:?}"),
            })
            .collect();
        assert_eq!(actual_deltas, *expected_deltas, "{response:?}");
        let PipelineEvent::TranslationFinal { text, timing, .. } = event else {
            panic!("Expected a final: {event:?}")
        };
        assert_eq!(text, strip_wrappers(response));
        assert_eq!(timing.speech_end_ms, 1000);
        assert_eq!(timing.asr_done_ms, 1100);
        assert_eq!(timing.queued_ms, 1200);
        assert_eq!(timing.sent_ms, 1400);
        assert_eq!(timing.first_token_ms, Some(*expected_first));
        assert_eq!(timing.done_ms, 1500 + fragments.len() as u64 * 100);
        let ordered = [
            timing.speech_end_ms,
            timing.asr_done_ms,
            timing.queued_ms,
            timing.sent_ms,
            timing.first_token_ms.unwrap(),
            timing.done_ms,
        ];
        assert!(ordered.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(
            (
                timing.prompt_tokens,
                timing.cached_tokens,
                timing.generated_tokens
            ),
            (17, 31, 9)
        );
    }
}

#[test]
fn incomplete_repeated_word_is_checked_only_once_its_boundary_or_final_arrives() {
    let cases: &[(&[&str], &str, &str)] = &[
        (
            &["Hello. a a a", "bout"],
            "Hello. a a about",
            "Hello. a a about",
        ),
        // The third repeated word is complete only at final output here.
        (&["Hello. the the th", "e"], "Hello. the the the", "Hello."),
    ];
    for (fragments, response, expected) in cases {
        let clock = ManualClock::new(StreamTime::from_millis(1400));
        let mut fake = ScriptedTranslator::new(clock.clone(), fragments, response);
        let reached_end = fake.reached_end.clone();
        let item = QueuedTranscript {
            transcript: transcript(1, "你好。", 1.0),
            queued_ms: 1200,
        };
        let event = translate_one(
            &mut fake,
            &item,
            &Config::default().translate,
            &clock,
            &EventBus::default(),
            Arc::new(AtomicBool::new(false)),
        );
        assert!(
            reached_end.load(Ordering::SeqCst),
            "Aborted an incomplete word in {response:?}"
        );
        assert!(
            matches!(event, PipelineEvent::TranslationFinal { text, .. } if text == *expected),
            "{response:?}"
        );
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

#[test]
fn empty_response_is_an_error_and_only_repetition_is_runaway() {
    for (response, expected) in [
        ("", FailReason::Error),
        ("\"\"", FailReason::Error),
        (" Translation: \"\" ", FailReason::Error),
        ("the the the", FailReason::Runaway),
    ] {
        let clock = ManualClock::new(StreamTime::from_millis(1400));
        let mut fake = ScriptedTranslator::new(clock.clone(), &[], response);
        let item = QueuedTranscript {
            transcript: transcript(1, "你好。", 1.0),
            queued_ms: 1200,
        };
        let event = translate_one(
            &mut fake,
            &item,
            &Config::default().translate,
            &clock,
            &EventBus::default(),
            Arc::new(AtomicBool::new(false)),
        );
        assert!(
            matches!(event, PipelineEvent::TranslationFailed { reason, .. } if reason == expected),
            "{response:?}"
        );
    }
}

#[test]
fn source_language_tags_are_normalized_for_translator_requests() {
    for (tag, expected) in [
        (None, "zh"),
        (Some(""), "zh"),
        (Some("<|ZH|>"), "zh"),
        (Some(" <|ja|> "), "ja"),
        (Some("ko"), "ko"),
    ] {
        let clock = ManualClock::new(StreamTime::from_millis(1400));
        let mut fake = ScriptedTranslator::new(clock.clone(), &["Hello."], "Hello.");
        fake.expected_source = expected.into();
        let mut source = transcript(1, "你好。", 1.0);
        source.lang_tag = tag.map(String::from);
        let item = QueuedTranscript {
            transcript: source,
            queued_ms: 1200,
        };
        assert!(matches!(
            translate_one(
                &mut fake,
                &item,
                &Config::default().translate,
                &clock,
                &EventBus::default(),
                Arc::new(AtomicBool::new(false))
            ),
            PipelineEvent::TranslationFinal { .. }
        ));
    }
}

#[test]
fn timeout_fake_preserves_pipeline_cancellation() {
    let cancelled = Arc::new(AtomicBool::new(true));
    let request = TranslateRequest {
        id: UtteranceId(1),
        text: "超时。",
        src: "zh",
        tgt: "en",
        terms: &[],
        context: &[],
        control: TranslationControl {
            deadline: Instant::now() + Duration::from_secs(1),
            cancelled,
            abort: Arc::new(AtomicBool::new(false)),
        },
    };
    let mut fake = FakeTranslator {
        timeout_text: Some("超时。".into()),
        ..FakeTranslator::default()
    };
    assert!(matches!(
        fake.translate(&request, &mut |_| {}),
        Err(Error::Stopped)
    ));
    assert_eq!(fake.metrics.active.load(Ordering::SeqCst), 0);
}

#[test]
fn shutdown_does_not_wait_for_a_full_completion_channel() {
    let fake = FakeTranslator::default();
    let metrics = fake.metrics.clone();
    let mut worker = TranslatorWorker::start(
        Box::new(fake),
        Config::default().translate,
        Arc::new(ManualClock::new(StreamTime::from_seconds(3.0))),
        EventBus::default(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    for id in 1..=2 {
        worker
            .input
            .send_timeout(
                QueuedTranscript {
                    transcript: transcript(id, "你好。", id as f64),
                    queued_ms: id * 1000,
                },
                Duration::from_secs(1),
            )
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    while (metrics.requests.load(Ordering::SeqCst) != 2
        || metrics.active.load(Ordering::SeqCst) != 0
        || worker.completed.len() != 1)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(metrics.requests.load(Ordering::SeqCst), 2);
    assert_eq!(metrics.active.load(Ordering::SeqCst), 0);
    assert_eq!(worker.completed.len(), 1);
    let start = Instant::now();
    worker.stop().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(metrics.active.load(Ordering::SeqCst), 0);
}
