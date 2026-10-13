//! End-to-end behavior of the pipeline threads with fake engines and no models: clause cuts,
//! live text, the second pass, translation, skipping, pause and resume, shutdown, backpressure
//! and the per-id event order.

use crossbeam_channel::{bounded, Receiver, Sender};
use lt_core::{
    bus::{EventBus, EventReceiver},
    config::Config,
    engines::{
        OfflineAsr, OfflineText, StreamResult, StreamingAsr, TokenTranscript, TranslateRequest,
        TranslationControl, TranslationOut, Translator, TranslatorCaps,
    },
    error::{Error, Result},
    events::{DropReason, EngineKind, EngineState, FailReason, ListeningStateKind, PipelineEvent},
    fakes::{FakeStream, FakeTranslator},
    pipeline::{Pipeline, PipelineHandle},
    queue::TranslationQueue,
    source::{AudioFrame, AudioProducer, AudioSource, FrameFlags, SourceEvents},
    types::{
        CaptureMode, CutReason, SecondPassOutcome, SourceInfo, StreamTime, TextClass, UtteranceId,
    },
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Only stops a hung test; no assertion depends on how fast this PC is.
const WATCHDOG: Duration = Duration::from_secs(30);

/// Sends its samples in 512-sample frames as fast as the pipeline takes them. With `hold_open`
/// it then stays open until stopped, like a sound device; otherwise the audio ends.
struct VecSource {
    samples: Arc<Vec<f32>>,
    label: String,
    hold_open: bool,
    cancelled: Option<Arc<AtomicBool>>,
    thread: Option<JoinHandle<()>>,
}

impl VecSource {
    fn new(samples: Vec<f32>) -> Self {
        Self {
            samples: Arc::new(samples),
            label: "Test audio".into(),
            hold_open: false,
            cancelled: None,
            thread: None,
        }
    }
}

impl AudioSource for VecSource {
    fn start(&mut self, producer: AudioProducer, _events: SourceEvents) -> Result<SourceInfo> {
        self.stop();
        self.cancelled = Some(producer.cancelled.clone());
        let samples = self.samples.clone();
        let hold_open = self.hold_open;
        self.thread = Some(thread::spawn(move || {
            for (index, chunk) in samples.chunks(512).enumerate() {
                let t0 = StreamTime(producer.base_time.samples() + index as u64 * 512);
                let mut frame = [0.0; 512];
                frame[..chunk.len()].copy_from_slice(chunk);
                let flags = FrameFlags::EMPTY;
                if producer
                    .send(AudioFrame {
                        t0,
                        samples: frame,
                        flags,
                    })
                    .is_err()
                {
                    return;
                }
            }
            while hold_open && !producer.cancelled.load(Ordering::Acquire) {
                thread::park_timeout(Duration::from_millis(5));
            }
        }));
        Ok(SourceInfo {
            mode: CaptureMode::System,
            label: self.label.clone(),
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

impl Drop for VecSource {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `phrases` bursts of a 1 kHz tone, each 8 frames long and followed by 13 silent frames: more
/// silence than `PAUSE_S`, so each burst is one clause of a `FakeStream`.
fn tones(phrases: usize) -> Vec<f32> {
    let mut samples = Vec::new();
    for _ in 0..phrases {
        samples.extend(
            (0..8 * 512)
                .map(|n| (n as f32 * std::f32::consts::TAU * 1000.0 / 16_000.0).sin() * 0.5),
        );
        samples.resize(samples.len() + 13 * 512, 0.0);
    }
    samples
}

const PAUSE_S: f64 = 0.25;

fn fake_stream(texts: &[&str]) -> FakeStream {
    FakeStream::new(
        texts.iter().map(|text| (*text).to_owned()).collect(),
        PAUSE_S,
    )
}

const SENTENCE: &str = "今天天气很好我们一起去公园玩，然后回家吃饭再看电影。接着继续聊天说话吧";
const TOKEN_SAMPLES: usize = 3_200;

fn sentence_chars() -> Vec<char> {
    SENTENCE.chars().collect()
}

/// `tokens` tokens of amplitude-coded speech followed by `pause_s` seconds of silence.
fn speech(tokens: usize, pause_s: f32) -> Vec<f32> {
    let mut samples = Vec::new();
    for index in 0..tokens {
        samples.extend(std::iter::repeat_n(
            0.05 + 0.0005 * index as f32,
            TOKEN_SAMPLES,
        ));
    }
    samples.extend(std::iter::repeat_n(0.0, (pause_s * 16_000.0) as usize));
    samples
}

/// Runs of constant amplitude at least 50 ms long: `(char, start_seconds)`. Without `complete`,
/// the last run counts only once the amplitude changed after it.
fn decode_runs(samples: &[f32], complete: bool) -> Vec<(char, f32)> {
    let chars = sentence_chars();
    let mut tokens = Vec::new();
    let mut start = 0;
    for end in 1..=samples.len() {
        let closed = end < samples.len() && (samples[end] - samples[start]).abs() > 1e-6;
        if closed || (end == samples.len() && complete) {
            let value = samples[start];
            if end - start >= 800 && value > 0.04 {
                let index = ((value - 0.05) / 0.0005).round() as usize;
                tokens.push((chars[index % chars.len()], start as f32 / 16_000.0));
            }
            start = end;
        }
    }
    tokens
}

/// A streaming recognizer over `speech` audio: each 200 ms run of one amplitude is the token at
/// that position of `SENTENCE`, and it appears when the run ends, so tokens follow the audio
/// like a real recognizer's. Its pause rule fires after `PAUSE_SAMPLES` of silence.
#[derive(Default)]
struct AmplitudeStream {
    part: Vec<f32>,
    heard: u64,
    part_start: u64,
}

const PAUSE_SAMPLES: usize = 6_400;

impl AmplitudeStream {
    fn tokens(&self, complete: bool) -> StreamResult {
        let runs = decode_runs(&self.part, complete);
        StreamResult {
            start_s: self.part_start as f64 / 16_000.0,
            tokens: TokenTranscript {
                text: runs.iter().map(|(c, _)| *c).collect(),
                tokens: runs.iter().map(|(c, _)| c.to_string()).collect(),
                timestamps: runs.iter().map(|(_, t)| *t).collect(),
                log_probs: Vec::new(),
            },
        }
    }
}

impl StreamingAsr for AmplitudeStream {
    fn accept(&mut self, samples: &[f32]) -> Result<()> {
        self.part.extend_from_slice(samples);
        self.heard += samples.len() as u64;
        Ok(())
    }
    fn result(&mut self) -> Result<StreamResult> {
        Ok(self.tokens(false))
    }
    fn is_endpoint(&mut self) -> bool {
        let silence = self.part.iter().rev().take_while(|s| **s == 0.0).count();
        silence >= PAUSE_SAMPLES && !decode_runs(&self.part, false).is_empty()
    }
    fn reset(&mut self) {
        self.part.clear();
        self.part_start = self.heard;
    }
    fn finish(&mut self) -> Result<StreamResult> {
        let result = self.tokens(true);
        *self = Self::default();
        Ok(result)
    }
}

/// Each request's text and the number of earlier lines sent with it.
type RequestLog = Arc<Mutex<Vec<(String, usize)>>>;

/// Records its requests and accepts earlier lines as context. With a gate, its first request
/// blocks until the gate opens or the pipeline cancels it.
struct RecordingTranslator {
    requests: RequestLog,
    gate: Option<(Sender<()>, Receiver<()>)>,
    fake: FakeTranslator,
}

impl RecordingTranslator {
    fn new() -> (Self, RequestLog) {
        let requests = RequestLog::default();
        let translator = Self {
            requests: requests.clone(),
            gate: None,
            fake: FakeTranslator::default(),
        };
        (translator, requests)
    }

    /// Also returns the sender that opens the gate and the receiver that tells when the first
    /// request is waiting at it.
    fn gated() -> (Self, RequestLog, Sender<()>, Receiver<()>) {
        let (mut translator, requests) = Self::new();
        let (open, gate) = bounded(1);
        let (entered, waiting) = bounded(1);
        translator.gate = Some((entered, gate));
        (translator, requests, open, waiting)
    }
}

impl Translator for RecordingTranslator {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            context: true,
            max_input_chars: 300,
        }
    }
    fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
        self.fake.warm_up(control)
    }
    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        self.requests
            .lock()
            .unwrap()
            .push((request.text.into(), request.context.len()));
        if let Some((entered, gate)) = self.gate.take() {
            entered.send(()).unwrap();
            loop {
                request.control.check()?;
                match gate.recv_timeout(Duration::from_millis(5)) {
                    Ok(()) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        return Err(Error::Stopped)
                    }
                }
            }
        }
        self.fake.translate(request, on_delta)
    }
}

/// A translator whose seeded rolls vary how long each request holds the worker thread and fail
/// some requests, so translations finish at varied points of the recognizer's progress.
struct SeededTranslator {
    state: u64,
    fail_percent: u64,
}

impl SeededTranslator {
    fn new(seed: u64, fail_percent: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            fail_percent,
        }
    }
    fn next(&mut self) -> u64 {
        // xorshift64
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }
}

impl Translator for SeededTranslator {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            context: false,
            max_input_chars: 300,
        }
    }
    fn warm_up(&mut self, _: &TranslationControl) -> Result<()> {
        Ok(())
    }
    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        let roll = self.next();
        for _ in 0..roll % 2_000 {
            thread::yield_now();
        }
        if (roll >> 20) % 100 < self.fail_percent {
            return Err(Error::Translation {
                reason: FailReason::ServerUnavailable,
                message: "scripted failure".into(),
            });
        }
        // One English word per two Chinese characters, streamed word by word.
        let words: Vec<String> = (0..request.text.chars().count() / 2 + 1)
            .map(|i| format!("w{i}"))
            .collect();
        for count in 1..=words.len() {
            on_delta(&words[..count].join(" "));
        }
        Ok(TranslationOut {
            text: words.join(" "),
            ..TranslationOut::default()
        })
    }
}

fn start(
    source: VecSource,
    asr: impl StreamingAsr + 'static,
    translator: impl Translator + 'static,
) -> (PipelineHandle, EventReceiver) {
    start_with(Config::default(), source, asr, None, translator)
}

fn start_with(
    config: Config,
    source: VecSource,
    asr: impl StreamingAsr + 'static,
    second_pass: Option<Box<dyn OfflineAsr>>,
    translator: impl Translator + 'static,
) -> (PipelineHandle, EventReceiver) {
    let bus = EventBus::default();
    let events = bus.subscribe(4096);
    let handle = Pipeline::start(
        config,
        Box::new(source),
        Box::new(asr),
        second_pass,
        Box::new(translator),
        bus,
        None,
    )
    .unwrap();
    (handle, events)
}

enum Reply {
    Fail,
    Text(&'static str),
    /// The text after this long.
    Slow(&'static str, Duration),
}

/// A second pass that gives its replies in call order. Each call reports its start.
struct ScriptedSecondPass {
    replies: VecDeque<Reply>,
    started: Sender<()>,
}

impl OfflineAsr for ScriptedSecondPass {
    fn decode(&mut self, samples: &[f32], _language: Option<&str>) -> Result<OfflineText> {
        assert!(!samples.is_empty(), "a window without audio");
        let _ = self.started.try_send(());
        let text = match self.replies.pop_front() {
            Some(Reply::Fail) => return Err(Error::Engine("decode failed".into())),
            Some(Reply::Text(text)) => text,
            Some(Reply::Slow(text, after)) => {
                thread::sleep(after);
                text
            }
            None => "",
        };
        Ok(OfflineText {
            text: text.into(),
            lang: Some("zh".into()),
        })
    }
}

fn second_pass_outcomes(events: &[PipelineEvent]) -> Vec<(String, Option<SecondPassOutcome>)> {
    events
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::AsrFinal {
                text, second_pass, ..
            } => Some((text.clone(), second_pass.as_ref().map(|pass| pass.outcome))),
            _ => None,
        })
        .collect()
}

/// Waits until a finite source has been heard and every line has settled.
fn finish(handle: &mut PipelineHandle) {
    let deadline = Instant::now() + WATCHDOG;
    while !handle.is_finished() {
        assert!(Instant::now() < deadline, "the pipeline did not finish");
        thread::park_timeout(Duration::from_millis(2));
    }
    handle.wait().unwrap();
}

/// Receives events into `recorded` until `predicate` holds.
fn record_until(
    events: &EventReceiver,
    recorded: &mut Vec<PipelineEvent>,
    predicate: impl Fn(&[PipelineEvent]) -> bool,
) {
    let deadline = Instant::now() + WATCHDOG;
    while !predicate(recorded) {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(event) => recorded.push(event),
            Err(error) => panic!("expected events did not arrive ({error}): {recorded:?}"),
        }
    }
}

fn count(events: &[PipelineEvent], matches: impl Fn(&PipelineEvent) -> bool) -> usize {
    events.iter().filter(|event| matches(event)).count()
}

fn is_final(event: &PipelineEvent) -> bool {
    matches!(event, PipelineEvent::AsrFinal { .. })
}

fn finals(events: &[PipelineEvent]) -> Vec<(UtteranceId, String, CutReason)> {
    events
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::AsrFinal { id, text, cut, .. } => Some((*id, text.clone(), *cut)),
            _ => None,
        })
        .collect()
}

/// The per-id order of docs/ipc.md: one start; live text only before the final; one final;
/// English and other-language finals, drops, skips, joins and translation results are terminal;
/// nothing follows a terminal event; and every line ends with one.
fn assert_order(events: &[PipelineEvent]) {
    #[derive(Default)]
    struct Line {
        started: bool,
        asr_final: bool,
        terminal: bool,
    }
    let mut lines: BTreeMap<UtteranceId, Line> = BTreeMap::new();
    for event in events {
        match event {
            PipelineEvent::SpeechStarted { id, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(!line.started, "repeated start: {id:?}");
                line.started = true;
            }
            PipelineEvent::AsrPartial { id, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(
                    !line.asr_final && !line.terminal,
                    "live text after the final: {id:?}"
                );
            }
            PipelineEvent::AsrFinal { id, class, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(!line.asr_final && !line.terminal, "repeated final: {id:?}");
                line.asr_final = true;
                line.terminal = !class.is_translated();
            }
            PipelineEvent::Dropped { id, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(
                    !line.asr_final && !line.terminal,
                    "drop after the final: {id:?}"
                );
                line.terminal = true;
            }
            PipelineEvent::Joined { id, absorbed, .. } => {
                let leader = &lines[id];
                assert!(
                    leader.asr_final && !leader.terminal,
                    "bad join leader: {id:?}"
                );
                for id in absorbed {
                    let line = lines.get_mut(id).expect("absorbed an unknown id");
                    assert!(line.asr_final && !line.terminal, "absorbed twice: {id:?}");
                    line.terminal = true;
                }
            }
            PipelineEvent::TranslationDelta { id, .. } => {
                let line = &lines[id];
                assert!(
                    line.asr_final && !line.terminal,
                    "delta after a terminal: {id:?}"
                );
            }
            PipelineEvent::TranslationFinal { id, .. }
            | PipelineEvent::TranslationFailed { id, .. }
            | PipelineEvent::Skipped { id, .. } => {
                let line = lines.get_mut(id).expect("terminal for an unknown id");
                assert!(line.asr_final && !line.terminal, "bad terminal: {id:?}");
                line.terminal = true;
            }
            _ => {}
        }
    }
    assert!(!lines.is_empty());
    let open: Vec<_> = lines
        .iter()
        .filter(|(_, line)| !line.terminal)
        .map(|(id, _)| id)
        .collect();
    assert!(
        open.is_empty(),
        "lines left without a terminal event: {open:?}"
    );
}

/// The sentence (one token each 200 ms) is cut at its comma once the clause is old enough, at its
/// sentence end and at the pause, and its Chinese shows while it is spoken.
#[test]
fn clauses_end_at_commas_sentence_ends_and_pauses_and_show_live_text() {
    let chars = sentence_chars();
    let (mut handle, events) = start(
        VecSource::new(speech(chars.len(), 1.0)),
        AmplitudeStream::default(),
        FakeTranslator::default(),
    );
    finish(&mut handle);
    let events: Vec<_> = events.try_iter().collect();
    assert_order(&events);
    let clauses = finals(&events);
    let texts: Vec<_> = clauses.iter().map(|(_, text, _)| text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "今天天气很好我们一起去公园玩，",
            "然后回家吃饭再看电影。",
            "接着继续聊天说话吧"
        ]
    );
    let cuts: Vec<_> = clauses.iter().map(|(_, _, cut)| *cut).collect();
    assert_eq!(
        cuts,
        [CutReason::Commit, CutReason::Commit, CutReason::Pause]
    );
    for (id, ..) in &clauses {
        assert!(
            events.iter().any(
                |event| matches!(event, PipelineEvent::AsrPartial { id: live, .. } if live == id)
            ),
            "no live text for {id:?}"
        );
    }
}

#[test]
fn chinese_is_translated_english_is_shown_as_heard_and_fillers_are_dropped() {
    let translator = FakeTranslator::default();
    let metrics = translator.metrics.clone();
    let (mut handle, events) = start(
        VecSource::new(tones(3)),
        fake_stream(&["你好。", "Okay I see.", "嗯。"]),
        translator,
    );
    finish(&mut handle);
    let events: Vec<_> = events.try_iter().collect();
    assert_order(&events);
    let classes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::AsrFinal { class, .. } => Some(*class),
            _ => None,
        })
        .collect();
    assert_eq!(classes, [TextClass::Chinese, TextClass::English]);
    assert_eq!(metrics.requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(&events, |event| matches!(
            event,
            PipelineEvent::Dropped {
                reason: DropReason::Empty,
                ..
            }
        )),
        1
    );
}

/// While the translator is busy, 13 lines wait. The oldest waited past `skip_lag_s`, so all but
/// the newest are skipped.
#[test]
fn lines_waiting_past_the_skip_lag_are_skipped_and_the_newest_is_translated() {
    let phrases: Vec<String> = (0..14)
        .map(|index| format!("我们说第{index}句话。"))
        .collect();
    let (translator, requests, open, waiting) = RecordingTranslator::gated();
    let (mut handle, events) = start(
        VecSource::new(tones(14)),
        FakeStream::new(phrases.clone(), PAUSE_S),
        translator,
    );
    waiting.recv_timeout(WATCHDOG).unwrap();
    let mut recorded = Vec::new();
    record_until(&events, &mut recorded, |events| {
        count(events, is_final) == 14
    });
    open.send(()).unwrap();
    finish(&mut handle);
    recorded.extend(events.try_iter());
    assert_order(&recorded);
    let texts: Vec<_> = requests
        .lock()
        .unwrap()
        .iter()
        .map(|(text, _)| text.clone())
        .collect();
    assert_eq!(texts, [phrases[0].clone(), phrases[13].clone()]);
    assert_eq!(
        count(&recorded, |event| matches!(
            event,
            PipelineEvent::Skipped { .. }
        )),
        12
    );
    let stats = handle.get_stats();
    assert_eq!(stats.skipped_total, 12);
    assert_eq!(stats.failed_total, 0);
    assert_eq!(stats.lag_ms, 0);
    assert_eq!(stats.queue_depth, 0);
    assert!(stats.done_p50_ms.is_some());
}

/// Pausing ends the open line, which is still translated. After a source change and resume, ids
/// keep counting up and the translation window starts over: the next line goes alone.
#[test]
fn pause_ends_the_open_line_and_resume_starts_a_new_window() {
    let (translator, requests) = RecordingTranslator::new();
    // Speech without the silence after it: only the pause can end this line.
    let mut device = VecSource::new(tones(1)[..8 * 512].to_vec());
    device.hold_open = true;
    let (mut handle, events) = start(device, fake_stream(&["你好。", "再见。"]), translator);
    let mut recorded = Vec::new();
    record_until(&events, &mut recorded, |events| {
        events
            .iter()
            .any(|event| matches!(event, PipelineEvent::SpeechStarted { .. }))
    });
    handle.pause().unwrap();
    record_until(&events, &mut recorded, |events| {
        let paused = events.iter().any(|event| {
            matches!(
                event,
                PipelineEvent::ListeningState {
                    state: ListeningStateKind::Paused
                }
            )
        });
        paused
            && events
                .iter()
                .any(|event| matches!(event, PipelineEvent::TranslationFinal { .. }))
    });
    assert_eq!(finals(&recorded)[0].2, CutReason::Discontinuity);
    let mut replacement = VecSource::new(tones(1));
    replacement.label = "Replacement".into();
    handle.reopen_source(Box::new(replacement)).unwrap();
    handle.resume().unwrap();
    finish(&mut handle);
    recorded.extend(events.try_iter());
    assert_order(&recorded);
    let ids: Vec<_> = finals(&recorded).iter().map(|(id, ..)| *id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids[1] > ids[0]);
    assert!(recorded.iter().any(|event| matches!(
        event,
        PipelineEvent::SourceChanged { info } if info.label == "Replacement"
    )));
    assert_eq!(
        *requests.lock().unwrap(),
        [("你好。".to_owned(), 0), ("再见。".to_owned(), 0)]
    );
}

/// Stop cancels the busy translation, joins every thread and settles every line.
#[test]
fn stop_settles_every_line_while_translation_is_busy() {
    let (translator, _requests, _open, waiting) = RecordingTranslator::gated();
    let (mut handle, events) = start(
        VecSource::new(tones(4)),
        FakeStream::new(vec!["你好。".into(); 4], PAUSE_S),
        translator,
    );
    waiting.recv_timeout(WATCHDOG).unwrap();
    let mut recorded = Vec::new();
    record_until(&events, &mut recorded, |events| {
        count(events, is_final) == 4
    });
    handle.stop().unwrap();
    assert!(handle.is_finished());
    recorded.extend(events.try_iter());
    assert_order(&recorded);
    let stats = handle.get_stats();
    assert_eq!(stats.lag_ms, 0);
    assert_eq!(stats.queue_depth, 0);
    assert!(stats.failed_total > 0);
}

/// Whatever the second pass does (fail, hear nothing, correct a word, take too long), every
/// line reaches the translator once and in order, and keeps the live text unless the second
/// pass's text is used.
#[test]
fn every_line_reaches_the_translator_once_and_in_order_whatever_the_second_pass_does() {
    let live = [
        "你好世界。",
        "我们走吧。",
        "他们来了。",
        "天气很好。",
        "明天见面。",
        "晚上吃饭。",
    ];
    let (started, _) = bounded(16);
    let second_pass = ScriptedSecondPass {
        replies: VecDeque::from([
            Reply::Fail,
            Reply::Text(""),
            Reply::Text("前面。他们来啦。后面"),
            // The lines behind this decode wait longer than the 0.5 s limit.
            Reply::Slow("天气很好。", Duration::from_millis(1_500)),
        ]),
        started,
    };
    let mut config = Config::default();
    config.asr.second_pass_max_wait_s = 0.5;
    let (mut handle, events) = start_with(
        config,
        VecSource::new(tones(live.len())),
        fake_stream(&live),
        Some(Box::new(second_pass)),
        FakeTranslator::default(),
    );
    finish(&mut handle);
    let events: Vec<_> = events.try_iter().collect();
    assert_order(&events);
    let ids: Vec<_> = finals(&events).iter().map(|(id, ..)| id.0).collect();
    assert_eq!(ids, [0, 1, 2, 3, 4, 5]);
    let expected = [
        ("你好世界。", SecondPassOutcome::Failed),
        ("我们走吧。", SecondPassOutcome::Mismatch),
        ("他们来啦。", SecondPassOutcome::Used),
        ("天气很好。", SecondPassOutcome::Used),
        ("明天见面。", SecondPassOutcome::Late),
        ("晚上吃饭。", SecondPassOutcome::Late),
    ];
    for (index, ((text, outcome), (expected_text, expected_outcome))) in
        second_pass_outcomes(&events)
            .into_iter()
            .zip(expected)
            .enumerate()
    {
        // A slow test machine can make an early line late too; it then keeps the live text.
        if outcome == Some(SecondPassOutcome::Late) {
            assert_eq!(text, live[index]);
        } else {
            assert_eq!(
                (text.as_str(), outcome),
                (expected_text, Some(expected_outcome))
            );
        }
    }
    let live_text = events.iter().find_map(|event| match event {
        PipelineEvent::AsrFinal {
            id: UtteranceId(2),
            second_pass: Some(pass),
            ..
        } => Some(pass.live_text.clone()),
        _ => None,
    });
    assert_eq!(live_text, Some(Some("他们来了。".into())));
}

/// Stop returns while the second pass decodes and lines wait for it, and every line ends.
#[test]
fn stop_joins_every_thread_while_the_second_pass_decodes() {
    let (started, decoding) = bounded(64);
    let second_pass = ScriptedSecondPass {
        replies: (0..40)
            .map(|_| Reply::Slow("你好。", Duration::from_millis(300)))
            .collect(),
        started,
    };
    let (mut handle, events) = start_with(
        Config::default(),
        VecSource::new(tones(40)),
        FakeStream::new(vec!["你好。".into(); 40], PAUSE_S),
        Some(Box::new(second_pass)),
        FakeTranslator::default(),
    );
    decoding.recv_timeout(WATCHDOG).unwrap();
    let (stopped_tx, stopped) = bounded(1);
    thread::spawn(move || {
        let result = handle.stop();
        let _ = stopped_tx.send((result, handle.is_finished()));
    });
    let (result, finished) = stopped.recv_timeout(WATCHDOG).expect("stop did not return");
    result.unwrap();
    assert!(finished);
    let recorded: Vec<_> = events.try_iter().collect();
    assert_order(&recorded);
}

/// A blocked translator stops the recognizer once the queue is full, instead of letting finals
/// pile up without bound, and pause and stop still work while everything waits.
#[test]
fn a_blocked_translator_bounds_the_lines_recognized_ahead() {
    let (translator, requests, _open, waiting) = RecordingTranslator::gated();
    let (mut handle, events) = start(
        VecSource::new(tones(300)),
        FakeStream::new(vec!["你好。".into(); 300], PAUSE_S),
        translator,
    );
    waiting.recv_timeout(WATCHDOG).unwrap();
    let mut recorded = Vec::new();
    record_until(&events, &mut recorded, |events| {
        count(events, is_final) >= TranslationQueue::CAPACITY
    });
    handle.pause().unwrap();
    handle.stop().unwrap();
    recorded.extend(events.try_iter());
    assert_order(&recorded);
    // One line at the translator, a full queue and one line between the queue and the recognizer.
    assert!(count(&recorded, is_final) <= TranslationQueue::CAPACITY + 2);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

/// A translator that panics.
struct PanickingTranslator;

impl Translator for PanickingTranslator {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            context: false,
            max_input_chars: 300,
        }
    }
    fn warm_up(&mut self, _: &TranslationControl) -> Result<()> {
        Ok(())
    }
    fn translate(
        &mut self,
        _: &TranslateRequest<'_>,
        _: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        panic!("scripted translator panic");
    }
}

/// A translator panic ends the session with an error instead of stalling it, and every line
/// still gets its terminal event.
#[test]
fn a_translator_panic_ends_the_session_and_settles_every_line() {
    let (mut handle, events) = start(
        VecSource::new(tones(3)),
        FakeStream::new(vec!["你好。".into(); 3], PAUSE_S),
        PanickingTranslator,
    );
    let deadline = Instant::now() + WATCHDOG;
    while !handle.is_finished() {
        assert!(Instant::now() < deadline, "the pipeline stalled");
        thread::park_timeout(Duration::from_millis(2));
    }
    assert!(handle.wait().is_err());
    let events: Vec<_> = events.try_iter().collect();
    assert_order(&events);
    assert!(events.iter().any(|event| matches!(
        event,
        PipelineEvent::EngineStatus {
            engine: EngineKind::Translator,
            state: EngineState::Failed,
            ..
        }
    )));
}

#[test]
fn random_translation_timing_and_failures_keep_every_ordering_rule() {
    let chars = sentence_chars();
    for seed in 1..=10_u64 {
        let mut samples = Vec::new();
        for round in 0..3 {
            samples.extend(speech(
                chars.len() - (seed as usize + round) % 6,
                0.5 + 0.1 * round as f32,
            ));
        }
        let (mut handle, events) = start(
            VecSource::new(samples),
            AmplitudeStream::default(),
            SeededTranslator::new(seed, 30),
        );
        finish(&mut handle);
        let events: Vec<_> = events.try_iter().collect();
        assert_order(&events);
        assert!(!finals(&events).is_empty(), "seed {seed}");
    }
}
