use crossbeam_channel::{bounded, Receiver, Sender};
use lt_core::{
    bus::{EventBus, EventReceiver},
    config::Config,
    engines::{TranslateRequest, TranslationControl, TranslationOut, Translator, TranslatorCaps},
    error::{Error, Result},
    events::{DropReason, ListeningStateKind, PipelineEvent, SourceStateKind},
    fakes::{FakeAsr, FakeTranslator, FakeVad},
    pipeline::{Pipeline, PipelineHandle},
    segment::FrameFlags,
    source::{AudioFrame, AudioProducer, AudioSource, SourceEvents},
    types::{CaptureMode, SourceInfo, StreamTime, TextClass, UtteranceId},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
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
