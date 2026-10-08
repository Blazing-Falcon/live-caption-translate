use crossbeam_channel::{bounded, Receiver, Sender};
use lt_core::{
    bus::{EventBus, EventReceiver},
    config::Config,
    engines::{
        SegmentAsr, TranslateRequest, TranslationControl, TranslationOut, Translator,
        TranslatorCaps,
    },
    error::{Error, Result},
    events::{
        DropReason, EngineKind, EngineState, FailReason, JoinKind, ListeningStateKind,
        PipelineEvent, SourceStateKind,
    },
    fakes::{FakeAsr, FakeTranslator, FakeVad},
    metrics::{ResourceSample, ResourceSampler},
    pipeline::{Pipeline, PipelineHandle},
    segment::FrameFlags,
    source::{AudioFrame, AudioProducer, AudioSource, SourceEvents},
    types::{CaptureMode, Segment, SourceInfo, StreamTime, TextClass, Transcript, UtteranceId},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// The fixture exercises actual WAV bytes and prepared-frame delivery without
/// a sound device, desktop window, filesystem path or downloaded test audio.
struct SyntheticWavSource {
    samples: Arc<Vec<f32>>,
    name: String,
    frame_delay: Duration,
    cancelled: Option<Arc<AtomicBool>>,
    thread: Option<JoinHandle<()>>,
    realtime: bool,
}

impl SyntheticWavSource {
    fn from_bytes(bytes: &[u8]) -> Self {
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            16_000
        );
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
        assert_eq!(&bytes[36..40], b"data");
        let samples = bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32768.0)
            .collect();
        Self {
            samples: Arc::new(samples),
            name: "Synthetic WAV".into(),
            frame_delay: Duration::ZERO,
            cancelled: None,
            thread: None,
            realtime: false,
        }
    }
}

impl AudioSource for SyntheticWavSource {
    fn start(&mut self, producer: AudioProducer, events: SourceEvents) -> Result<SourceInfo> {
        self.stop();
        self.cancelled = Some(producer.cancelled.clone());
        let samples = self.samples.clone();
        let delay = self.frame_delay;
        let realtime = self.realtime;
        self.thread = Some(
            thread::Builder::new()
                .name("test-wav-adapter".into())
                .spawn(move || {
                    events.publish(PipelineEvent::SourceState {
                        state: SourceStateKind::Playing,
                        detail: None,
                    });
                    for (index, chunk) in samples.chunks(512).enumerate() {
                        let t0 = StreamTime(producer.base_time.samples() + index as u64 * 512);
                        if realtime && producer.wait_until_frame_end(t0).is_err() {
                            return;
                        }
                        let deadline = Instant::now() + delay;
                        while Instant::now() < deadline {
                            if producer.cancelled.load(Ordering::Acquire) {
                                return;
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                        let mut frame = [0.0; 512];
                        frame[..chunk.len()].copy_from_slice(chunk);
                        if producer
                            .send(AudioFrame {
                                t0,
                                samples: frame,
                                flags: FrameFlags::EMPTY,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                })?,
        );
        Ok(SourceInfo {
            mode: CaptureMode::System,
            label: self.name.clone(),
            sample_rate: 16_000,
            channels: 1,
        })
    }

    fn stop(&mut self) {
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Release);
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl Drop for SyntheticWavSource {
    fn drop(&mut self) {
        self.stop();
    }
}

fn wav(phrases: usize) -> Vec<u8> {
    let mut pcm = Vec::new();
    for _ in 0..phrases {
        // Eight speech frames satisfy the default 0.25 s minimum; thirteen
        // silence frames satisfy the 0.4 s close. Speech is a 1 kHz sine wave.
        for sample in 0..8 * 512 {
            let tone = (sample as f64 * std::f64::consts::TAU * 1000.0 / 16_000.0).sin();
            pcm.extend_from_slice(&((tone * 16_000.0) as i16).to_le_bytes());
        }
        pcm.resize(pcm.len() + 13 * 512 * 2, 0);
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&16_000_u32.to_le_bytes());
    bytes.extend_from_slice(&32_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&pcm);
    bytes
}

fn finish(handle: &mut PipelineHandle) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(handle.is_finished(), "Finite replay did not drain");
    handle.wait().unwrap();
}

fn next_until(
    events: &EventReceiver,
    recorded: &mut Vec<PipelineEvent>,
    predicate: impl Fn(&[PipelineEvent]) -> bool,
) {
    next_until_with_timeout(events, recorded, Duration::from_secs(2), predicate)
}

fn next_until_with_timeout(
    events: &EventReceiver,
    recorded: &mut Vec<PipelineEvent>,
    timeout: Duration,
    predicate: impl Fn(&[PipelineEvent]) -> bool,
) {
    let deadline = Instant::now() + timeout;
    while !predicate(recorded) && Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(event) => recorded.push(event),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("Unexpected event channel closure: {error}"),
        }
    }
    assert!(
        predicate(recorded),
        "Expected event did not arrive: {recorded:?}"
    );
}

#[derive(Default)]
struct Line {
    asr: bool,
    terminal: bool,
}

fn assert_terminal_order(events: &[PipelineEvent], enabled_other: &[&str]) {
    let mut lines: BTreeMap<UtteranceId, Line> = BTreeMap::new();
    for event in events {
        match event {
            PipelineEvent::SpeechStarted { id, .. } => {
                assert!(!lines.contains_key(id), "Repeated start: {id:?}");
                lines.insert(*id, Line::default());
            }
            PipelineEvent::AsrFinal {
                id, class, lang, ..
            } => {
                let line = lines.entry(*id).or_default();
                assert!(!line.asr && !line.terminal, "ASR after terminal: {id:?}");
                line.asr = true;
                line.terminal = *class == TextClass::English
                    || (*class == TextClass::Other
                        && !lang
                            .as_deref()
                            .is_some_and(|lang| enabled_other.contains(&lang)));
            }
            PipelineEvent::Joined { id, absorbed, .. } => {
                let leader = lines.get(id).unwrap();
                assert!(leader.asr && !leader.terminal);
                for id in absorbed {
                    let line = lines.get_mut(id).unwrap();
                    assert!(
                        line.asr && !line.terminal,
                        "Absorbed more than once: {id:?}"
                    );
                    line.terminal = true;
                }
            }
            PipelineEvent::TranslationDelta { id, .. } => {
                let line = lines.get(id).unwrap();
                assert!(line.asr && !line.terminal, "Delta after terminal: {id:?}");
            }
            PipelineEvent::TranslationFinal { id, .. }
            | PipelineEvent::TranslationFailed { id, .. }
            | PipelineEvent::Skipped { id, .. } => {
                let line = lines.get_mut(id).unwrap();
                assert!(line.asr && !line.terminal, "Repeated terminal: {id:?}");
                line.terminal = true;
            }
            PipelineEvent::Dropped { id, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(
                    !line.asr && !line.terminal,
                    "Drop after ASR or terminal: {id:?}"
                );
                line.terminal = true;
            }
            _ => {}
        }
    }
    assert!(!lines.is_empty());
    assert!(
        lines.values().all(|line| line.terminal),
        "Utterances left pending: {:?}; events: {events:?}",
        lines
            .iter()
            .filter_map(|(id, line)| (!line.terminal).then_some(id))
            .collect::<Vec<_>>()
    );
}

#[test]
fn wav_replay_routes_chinese_english_other_and_drops_fillers_before_asr_final() {
    let bus = EventBus::default();
    let events = bus.subscribe(128);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let asr = FakeAsr::with_metadata(vec![
        (
            "<|zh|>你好。".into(),
            Some("<|zh|>".into()),
            Some("<|Speech|>".into()),
        ),
        (
            "Okay I see.".into(),
            Some("en".into()),
            Some("Speech".into()),
        ),
        (
            "こんにちは。".into(),
            Some("ja".into()),
            Some("Speech".into()),
        ),
        ("嗯。".into(), Some("zh".into()), Some("Speech".into())),
    ]);
    let translator = FakeTranslator::default();
    let metrics = translator.metrics.clone();
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(4))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(asr),
        Box::new(translator),
        bus,
    )
    .unwrap();
    finish(&mut handle);
    let recorded: Vec<_> = events.try_iter().collect();
    let classes: Vec<_> = recorded
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::AsrFinal { class, .. } => Some(*class),
            _ => None,
        })
        .collect();
    assert_eq!(
        classes,
        [TextClass::Chinese, TextClass::English, TextClass::Other]
    );
    assert_eq!(metrics.requests.load(Ordering::SeqCst), 1);
    assert!(recorded.iter().any(|event| matches!(
        event,
        PipelineEvent::Dropped {
            reason: DropReason::Empty,
            ..
        }
    )));
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn eof_waits_for_successor_asr_and_keeps_the_short_phrase_hold_join() {
    let bus = EventBus::default();
    let events = bus.subscribe(128);
    let mut translator = FakeTranslator::default();
    translator.responses.insert(
        "我也想办一个，伟大的公司。".into(),
        "I also want to run a great company.".into(),
    );
    let metrics = translator.metrics.clone();
    let mut handle = Pipeline::start_with_bus(
        Config::default(),
        Box::new(SyntheticWavSource::from_bytes(&wav(2))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(vec![
            "我也想办一个。".into(),
            "伟大的公司。".into(),
        ])),
        Box::new(translator),
        bus,
    )
    .unwrap();
    finish(&mut handle);
    let recorded: Vec<_> = events.try_iter().collect();
    assert_eq!(metrics.requests.load(Ordering::SeqCst), 1);
    assert!(recorded.iter().any(|event| matches!(event, PipelineEvent::Joined { kind: JoinKind::Hold, text, absorbed, .. } if text == "我也想办一个，伟大的公司。" && absorbed.len() == 1)));
    assert!(recorded.iter().any(|event| matches!(event, PipelineEvent::TranslationFinal { text, .. } if text == "I also want to run a great company.")));
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn enabled_other_preserves_its_class_and_resolves_engine_language_aliases() {
    for (text, tag, language) in [
        ("こんにちは。", Some("<|ja|>"), "ja"),
        ("こんにちは。", Some("ja-JP"), "ja"),
        ("こんにちは。", Some("jpn"), "ja"),
        ("こんにちは。", Some("Japanese"), "ja"),
        ("こんにちは。", None, "ja"),
        ("안녕하세요.", Some("ko-KR"), "ko"),
        ("안녕하세요.", None, "ko"),
    ] {
        let bus = EventBus::default();
        let events = bus.subscribe(64);
        let mut config = Config::default();
        config.routing.translate_other = vec![language.into()];
        let asr = FakeAsr::with_metadata(vec![(text.into(), tag.map(String::from), None)]);
        let translator = FakeTranslator::default();
        let metrics = translator.metrics.clone();
        let mut handle = Pipeline::start_with_bus(
            config,
            Box::new(SyntheticWavSource::from_bytes(&wav(1))),
            Box::new(FakeVad::from_energy(0.001)),
            Box::new(asr),
            Box::new(translator),
            bus,
        )
        .unwrap();
        finish(&mut handle);
        let recorded: Vec<_> = events.try_iter().collect();
        assert_eq!(metrics.requests.load(Ordering::SeqCst), 1, "{tag:?}");
        assert!(recorded.iter().any(|event| matches!(event, PipelineEvent::AsrFinal { class: TextClass::Other, lang: Some(lang), .. } if lang == language)));
        assert_terminal_order(&recorded, &[language]);
    }
}

type RequestLog = Arc<Mutex<Vec<String>>>;

struct GatedTranslator {
    gate: Receiver<()>,
    entered: Sender<()>,
    requests: RequestLog,
    first: bool,
    fake: FakeTranslator,
}

impl Translator for GatedTranslator {
    fn caps(&self) -> TranslatorCaps {
        self.fake.caps()
    }
    fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
        self.fake.warm_up(control)
    }
    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        self.requests.lock().unwrap().push(request.text.into());
        if self.first {
            self.first = false;
            let _ = self.entered.try_send(());
            loop {
                request.control.check()?;
                match self.gate.recv_timeout(Duration::from_millis(5)) {
                    Ok(()) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        return Err(Error::Stopped)
                    }
                }
            }
            // ASR-final delivery precedes queue insertion. Let the independent
            // scheduler consume the last message before this completion.
            let deadline = Instant::now() + Duration::from_millis(30);
            while Instant::now() < deadline {
                request.control.check()?;
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.fake.translate(request, on_delta)
    }
}

fn gated() -> (GatedTranslator, Sender<()>, Receiver<()>, RequestLog) {
    let (release, gate) = bounded(1);
    let (entered, started) = bounded(1);
    let requests = Arc::new(Mutex::new(Vec::new()));
    (
        GatedTranslator {
            gate,
            entered,
            requests: requests.clone(),
            first: true,
            fake: FakeTranslator::default(),
        },
        release,
        started,
        requests,
    )
}

#[test]
fn waiting_transcripts_join_after_the_active_request_without_repeating_terminals() {
    let bus = EventBus::default();
    let events = bus.subscribe(256);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let (translator, release, started, requests) = gated();
    let phrases = ["第一句话。", "第二句话。", "第三句话。", "第四句话。"];
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(4))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(phrases.map(String::from).to_vec())),
        Box::new(translator),
        bus,
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut recorded = Vec::new();
    next_until(&events, &mut recorded, |events| {
        events
            .iter()
            .filter(|event| matches!(event, PipelineEvent::AsrFinal { .. }))
            .count()
            == 4
    });
    release.send(()).unwrap();
    finish(&mut handle);
    recorded.extend(events.try_iter());
    assert_eq!(
        *requests.lock().unwrap(),
        ["第一句话。", "第二句话，第三句话，第四句话。"]
    );
    assert!(recorded.iter().any(|event| matches!(event, PipelineEvent::Joined { kind: JoinKind::Queue, absorbed, .. } if absorbed.len() == 2)));
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn catch_up_skips_waiting_items_and_translates_the_newest_after_a_busy_request() {
    let bus = EventBus::default();
    let events = bus.subscribe(512);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let (translator, release, started, requests) = gated();
    let phrases: Vec<String> = (0..14)
        .map(|index| format!("我们说第{index}句话。"))
        .collect();
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(14))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(phrases.clone())),
        Box::new(translator),
        bus,
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut recorded = Vec::new();
    next_until(&events, &mut recorded, |events| {
        events
            .iter()
            .filter(|event| matches!(event, PipelineEvent::AsrFinal { .. }))
            .count()
            == 14
    });
    release.send(()).unwrap();
    finish(&mut handle);
    recorded.extend(events.try_iter());
    assert_eq!(
        *requests.lock().unwrap(),
        [phrases[0].clone(), phrases[13].clone()]
    );
    assert_eq!(
        recorded
            .iter()
            .filter(|event| matches!(event, PipelineEvent::Skipped { .. }))
            .count(),
        12
    );
    let stats = handle.get_stats();
    assert_eq!(stats.skipped_total, 12);
    assert_eq!(stats.failed_total, 0);
    assert_eq!(stats.lag_ms, 0);
    assert_eq!(stats.queue_depth, 0);
    assert!(stats.done_p50_ms.is_some());
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn an_echo_failure_is_terminal_and_the_next_request_can_complete() {
    let bus = EventBus::default();
    let events = bus.subscribe(128);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    // A low queue join cap keeps the two independent requests separate.
    config.translate.queue_join_max_chars = 20;
    let first = "第一段话含有足够多的汉字来保持单独翻译。";
    let second = "第二段话也含有足够多的汉字来保持单独翻译。";
    let mut translator = FakeTranslator::default();
    translator.responses.insert(first.into(), first.into());
    translator
        .responses
        .insert(second.into(), "The second line completed.".into());
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(2))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(vec![first.into(), second.into()])),
        Box::new(translator),
        bus,
    )
    .unwrap();
    finish(&mut handle);
    let recorded: Vec<_> = events.try_iter().collect();
    assert!(recorded.iter().any(|event| matches!(
        event,
        PipelineEvent::TranslationFailed {
            reason: FailReason::Echo,
            ..
        }
    )));
    assert!(recorded.iter().any(|event| matches!(event, PipelineEvent::TranslationFinal { text, .. } if text == "The second line completed.")));
    let stats = handle.get_stats();
    assert_eq!(stats.failed_total, 1);
    assert_eq!(stats.lag_ms, 0);
    assert_eq!(stats.queue_depth, 0);
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn pause_finishes_pending_translation_and_resume_preserves_ids_and_source_changes() {
    let bus = EventBus::default();
    let events = bus.subscribe(256);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let mut source = SyntheticWavSource::from_bytes(&wav(10));
    source.frame_delay = Duration::from_millis(3);
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(source),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(vec!["你好。".into(), "再见。".into()])),
        Box::new(FakeTranslator::default()),
        bus,
    )
    .unwrap();
    let mut recorded = Vec::new();
    next_until(&events, &mut recorded, |events| {
        events
            .iter()
            .any(|event| matches!(event, PipelineEvent::SpeechStarted { .. }))
    });
    handle.pause().unwrap();
    next_until(&events, &mut recorded, |events| {
        events
            .iter()
            .any(|event| matches!(event, PipelineEvent::TranslationFinal { .. }))
    });
    assert!(!handle.is_finished());
    assert!(recorded.iter().any(|event| matches!(
        event,
        PipelineEvent::ListeningState {
            state: ListeningStateKind::Paused
        }
    )));
    let mut replacement = SyntheticWavSource::from_bytes(&wav(1));
    replacement.name = "Replacement WAV".into();
    handle.reopen_source(Box::new(replacement)).unwrap();
    handle.resume().unwrap();
    finish(&mut handle);
    recorded.extend(events.try_iter());
    let ids: Vec<_> = recorded
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::AsrFinal { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids[1] > ids[0]);
    assert!(recorded.iter().any(|event| matches!(event, PipelineEvent::SourceChanged { info } if info.label == "Replacement WAV")));
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn a_slow_subscriber_does_not_delay_thirty_seconds_of_fast_wav_replay() {
    let bus = EventBus::default();
    let events = bus.subscribe(1024);
    let slow = bus.subscribe(1);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let source = SyntheticWavSource::from_bytes(&wav(45));
    assert!(source.samples.len() as f64 / 16_000.0 >= 30.0);
    let translator = FakeTranslator::default();
    let metrics = translator.metrics.clone();
    let start = Instant::now();
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(source),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(vec!["Okay I see.".into(); 45])),
        Box::new(translator),
        bus,
    )
    .unwrap();
    finish(&mut handle);
    assert!(start.elapsed() < Duration::from_secs(2));
    let recorded: Vec<_> = events.try_iter().collect();
    assert_eq!(
        recorded
            .iter()
            .filter(|event| matches!(
                event,
                PipelineEvent::AsrFinal {
                    class: TextClass::English,
                    ..
                }
            ))
            .count(),
        45
    );
    assert_eq!(metrics.requests.load(Ordering::SeqCst), 0);
    assert!(slow.try_recv().is_ok());
    assert!(matches!(
        slow.try_recv(),
        Err(crossbeam_channel::TryRecvError::Disconnected)
    ));
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn stop_joins_every_worker_while_translation_is_busy_and_settles_pending_lines() {
    let bus = EventBus::default();
    let events = bus.subscribe(512);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let (translator, _release, started, _) = gated();
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(4))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(vec!["你好。".into(); 4])),
        Box::new(translator),
        bus,
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut recorded = Vec::new();
    next_until(&events, &mut recorded, |events| {
        events
            .iter()
            .filter(|event| matches!(event, PipelineEvent::AsrFinal { .. }))
            .count()
            == 4
    });
    let start = Instant::now();
    handle.stop().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(handle.is_finished());
    let stats = handle.get_stats();
    assert_eq!(stats.lag_ms, 0);
    assert_eq!(stats.queue_depth, 0);
    assert_eq!(stats.held, 0);
    assert!(stats.failed_total > 0);
    recorded.extend(events.try_iter());
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn saturated_translation_backpressures_asr_and_pause_stop_remain_responsive() {
    let bus = EventBus::default();
    let events = bus.subscribe(2048);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let (translator, _release, started, requests) = gated();
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(300))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(vec!["你好。".into(); 300])),
        Box::new(translator),
        bus,
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut recorded = Vec::new();
    next_until_with_timeout(&events, &mut recorded, Duration::from_secs(5), |events| {
        events
            .iter()
            .filter(|event| matches!(event, PipelineEvent::AsrFinal { .. }))
            .count()
            >= 255
    });
    thread::sleep(Duration::from_millis(30));
    recorded.extend(events.try_iter());
    let count = recorded
        .iter()
        .filter(|event| matches!(event, PipelineEvent::AsrFinal { .. }))
        .count();
    assert!(count < 300, "ASR ignored the bounded translation backlog");
    assert_eq!(requests.lock().unwrap().len(), 1);
    let start = Instant::now();
    handle.pause().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    let start = Instant::now();
    handle.stop().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    recorded.extend(events.try_iter());
    assert_terminal_order(&recorded, &[]);
}

struct FailingSource {
    inner: SyntheticWavSource,
    stops: Arc<AtomicUsize>,
}

impl AudioSource for FailingSource {
    fn start(&mut self, producer: AudioProducer, events: SourceEvents) -> Result<SourceInfo> {
        // Simulate failure after an adapter thread was already created.
        self.inner.start(producer, events)?;
        Err(Error::Engine("Fixture capture startup failed".into()))
    }

    fn stop(&mut self) {
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.inner.stop();
    }
}

#[test]
fn capture_startup_failure_stops_the_partial_adapter_and_joins_pipeline_workers() {
    let stops = Arc::new(AtomicUsize::new(0));
    let source = FailingSource {
        inner: SyntheticWavSource::from_bytes(&wav(4)),
        stops: stops.clone(),
    };
    let start = Instant::now();
    let result = Pipeline::start(
        Config::default(),
        Box::new(source),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(Vec::new())),
        Box::new(FakeTranslator::default()),
    );
    assert!(
        matches!(result, Err(Error::Engine(message)) if message.contains("Fixture capture startup failed"))
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(stops.load(Ordering::SeqCst) >= 1);
}

struct FixedSampler(Arc<AtomicUsize>);

impl ResourceSampler for FixedSampler {
    fn sample(&mut self) -> ResourceSample {
        self.0.fetch_add(1, Ordering::SeqCst);
        ResourceSample {
            cpu_app_pct: 7.5,
            cpu_translator_pct: 150.0,
            rss_app_mb: 42,
            rss_translator_mb: 1300,
            cpu_system_pct: 0.0,
            cpu_draft_pct: 0.0,
            rss_draft_mb: 0,
        }
    }
}

#[test]
fn fast_replay_refreshes_latest_final_latency_and_injected_resources_without_forcing_stats() {
    let bus = EventBus::default();
    let events = bus.subscribe(128);
    let samples = Arc::new(AtomicUsize::new(0));
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let mut handle = Pipeline::start_with_bus_and_sampler(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(1))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(vec!["你好。".into()])),
        Box::new(FakeTranslator::default()),
        bus,
        Some(Box::new(FixedSampler(samples.clone()))),
    )
    .unwrap();
    finish(&mut handle);
    let recorded: Vec<_> = events.try_iter().collect();
    assert!(!recorded
        .iter()
        .any(|event| matches!(event, PipelineEvent::Stats(_))));
    let timing = recorded
        .iter()
        .find_map(|event| match event {
            PipelineEvent::TranslationFinal { timing, .. } => Some(timing),
            _ => None,
        })
        .unwrap();
    let stats = handle.get_stats();
    assert_eq!(
        stats.done_p50_ms,
        Some(timing.done_ms.saturating_sub(timing.speech_end_ms) as u32)
    );
    assert_eq!(
        stats.first_p50_ms,
        timing
            .first_token_ms
            .map(|first| first.saturating_sub(timing.speech_end_ms) as u32)
    );
    assert_eq!(stats.done_p95_ms, stats.done_p50_ms);
    assert_eq!(stats.lag_ms, 0);
    assert_eq!(stats.queue_depth, 0);
    assert_eq!(
        (
            stats.cpu_app_pct,
            stats.cpu_translator_pct,
            stats.rss_app_mb,
            stats.rss_translator_mb
        ),
        (7.5, 150.0, 42, 1300)
    );
    assert_eq!(samples.load(Ordering::SeqCst), 1);
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn real_time_frames_and_stats_follow_wall_time_and_pause_excludes_stats() {
    let bus = EventBus::default();
    let events = bus.subscribe(256);
    let samples = Arc::new(AtomicUsize::new(0));
    let mut source = SyntheticWavSource::from_bytes(&wav(20));
    source.realtime = true;
    let started = Instant::now();
    let mut handle = Pipeline::start_with_bus_and_sampler(
        Config::default(),
        Box::new(source),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::new(vec!["Okay I see.".into(); 20])),
        Box::new(FakeTranslator::default()),
        bus,
        Some(Box::new(FixedSampler(samples.clone()))),
    )
    .unwrap();
    let mut recorded = Vec::new();
    let mut stats_times = Vec::new();
    let mut first_speech = None;
    let deadline = started + Duration::from_secs(3);
    while stats_times.len() < 2 && Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(event) => {
                match &event {
                    PipelineEvent::SpeechStarted { .. } => {
                        first_speech.get_or_insert_with(|| started.elapsed());
                    }
                    PipelineEvent::Stats(stats) => {
                        stats_times.push(started.elapsed());
                        assert_eq!(stats.rss_app_mb, 42);
                        assert_eq!(stats.cpu_translator_pct, 150.0);
                    }
                    _ => {}
                }
                recorded.push(event);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("Unexpected event closure: {error}"),
        }
    }
    assert_eq!(stats_times.len(), 2);
    // Opening needs eight complete 32 ms frames; the source must not deliver
    // the first frame at t0 and bias end-of-speech latency by one frame.
    assert!(first_speech.unwrap() >= Duration::from_millis(250));
    assert!(stats_times[0] >= Duration::from_millis(950));
    assert!(stats_times[1] - stats_times[0] >= Duration::from_millis(950));
    handle.pause().unwrap();
    let pause_window = Instant::now() + Duration::from_millis(1100);
    while Instant::now() < pause_window {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(event) => recorded.push(event),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("Unexpected event closure: {error}"),
        }
    }
    let paused = recorded
        .iter()
        .position(|event| {
            matches!(
                event,
                PipelineEvent::ListeningState {
                    state: ListeningStateKind::Paused
                }
            )
        })
        .unwrap();
    assert!(!recorded[paused + 1..]
        .iter()
        .any(|event| matches!(event, PipelineEvent::Stats(_))));
    let queries = samples.load(Ordering::SeqCst);
    assert!(queries >= 3);
    assert!(queries <= started.elapsed().as_secs() as usize + 1);
    handle.stop().unwrap();
    recorded.extend(events.try_iter());
    assert_eq!(handle.get_stats().lag_ms, 0);
    assert_terminal_order(&recorded, &[]);
}

struct WaitingAsr {
    entered: Sender<StreamTime>,
    release: Receiver<()>,
    fake: FakeAsr,
}

impl SegmentAsr for WaitingAsr {
    fn transcribe(&mut self, segment: &Segment) -> Result<Transcript> {
        self.entered.try_send(segment.end).unwrap();
        self.release
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| Error::Engine("Fixture ASR was not released".into()))?;
        self.fake.transcribe(segment)
    }
}

#[test]
fn closed_segment_lag_is_visible_while_asr_is_pending_then_clears_on_final() {
    let bus = EventBus::default();
    let events = bus.subscribe(128);
    let (entered, waiting) = bounded(1);
    let (release, gate) = bounded(1);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let asr = WaitingAsr {
        entered,
        release: gate,
        fake: FakeAsr::new(vec!["你好。".into()]),
    };
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(1))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(asr),
        Box::new(FakeTranslator::default()),
        bus,
    )
    .unwrap();
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    let deadline = Instant::now() + Duration::from_millis(300);
    while handle.get_stats().lag_ms == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(handle.get_stats().lag_ms > 0);
    let mut recorded: Vec<_> = events.try_iter().collect();
    assert!(!recorded
        .iter()
        .any(|event| matches!(event, PipelineEvent::AsrFinal { .. })));
    release.send(()).unwrap();
    finish(&mut handle);
    recorded.extend(events.try_iter());
    let stats = handle.get_stats();
    assert_eq!(stats.lag_ms, 0);
    assert_eq!(stats.queue_depth, 0);
    assert!(stats.done_p50_ms.is_some());
    assert_terminal_order(&recorded, &[]);
}

#[test]
fn enabled_other_language_stays_in_stats_until_translation_finishes() {
    let bus = EventBus::default();
    let events = bus.subscribe(128);
    let mut config = Config::default();
    config.routing.translate_other = vec!["Japanese".into()];
    let (translator, release, started, _) = gated();
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(1))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(FakeAsr::with_metadata(vec![(
            "こんにちは。".into(),
            None,
            None,
        )])),
        Box::new(translator),
        bus,
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(handle.get_stats().lag_ms > 0);
    release.send(()).unwrap();
    finish(&mut handle);
    let recorded: Vec<_> = events.try_iter().collect();
    assert_eq!(handle.get_stats().lag_ms, 0);
    assert!(handle.get_stats().done_p50_ms.is_some());
    assert_terminal_order(&recorded, &["ja"]);
}

struct FailOnceAsr {
    failed: bool,
    fake: FakeAsr,
}

impl SegmentAsr for FailOnceAsr {
    fn transcribe(&mut self, segment: &Segment) -> Result<Transcript> {
        if !self.failed {
            self.failed = true;
            return Err(Error::Engine("Temporary fixture decode failure".into()));
        }
        self.fake.transcribe(segment)
    }
}

#[test]
fn asr_engine_recovers_its_ready_status_after_a_transient_failure() {
    let bus = EventBus::default();
    let events = bus.subscribe(128);
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    let asr = FailOnceAsr {
        failed: false,
        fake: FakeAsr::new(vec!["再见。".into()]),
    };
    let mut handle = Pipeline::start_with_bus(
        config,
        Box::new(SyntheticWavSource::from_bytes(&wav(2))),
        Box::new(FakeVad::from_energy(0.001)),
        Box::new(asr),
        Box::new(FakeTranslator::default()),
        bus,
    )
    .unwrap();
    finish(&mut handle);
    let recorded: Vec<_> = events.try_iter().collect();
    let status: Vec<_> = recorded
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::EngineStatus {
                engine: EngineKind::Asr,
                state,
                ..
            } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(
        status,
        [EngineState::Ready, EngineState::Failed, EngineState::Ready]
    );
    assert_eq!(handle.get_stats().lag_ms, 0);
    assert!(handle.get_stats().done_p50_ms.is_some());
    assert_terminal_order(&recorded, &[]);
}
