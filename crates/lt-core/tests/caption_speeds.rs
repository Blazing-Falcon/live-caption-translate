//! End-to-end behavior with fake engines and no models: early commit, live partials,
//! drafts, and the event ordering rules.
//!
//! The fake recognizer reads its text from the audio itself: every 200 ms of speech is one
//! token whose amplitude encodes its position in a fixed sentence, so its tokens follow the
//! audio like a real streaming recognizer's would.

use lt_core::{
    bus::EventBus,
    config::Config,
    engines::{
        StreamResult, StreamingAsr, TokenTranscript, TranslateRequest, TranslationControl,
        TranslationOut, Translator, TranslatorCaps,
    },
    error::{Error, Result},
    events::{FailReason, PipelineEvent},
    fakes::FakeTranslator,
    pipeline::{Pipeline, PipelineOptions},
    source::{AudioFrame, AudioProducer, AudioSource, FrameFlags, SourceEvents},
    types::{CaptureMode, CutReason, EffectiveMode, SourceInfo, StreamTime, UtteranceId},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const SENTENCE: &str = "今天天气很好我们一起去公园玩，然后回家吃饭再看电影。接着继续聊天说话吧";
const TOKEN_SAMPLES: usize = 3_200;

fn sentence_chars() -> Vec<char> {
    SENTENCE.chars().collect()
}

fn amplitude(index: usize) -> f32 {
    0.05 + 0.0005 * index as f32
}

/// `tokens` tokens of speech followed by `pause_s` seconds of silence.
fn speech(tokens: usize, pause_s: f32) -> Vec<f32> {
    let mut samples = Vec::new();
    for index in 0..tokens {
        samples.extend(std::iter::repeat_n(amplitude(index), TOKEN_SAMPLES));
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

/// A streaming recognizer over the amplitude-coded audio: a token appears when its run of
/// constant amplitude ends. Its pause rule fires after `PAUSE_SAMPLES` of silence after a token.
#[derive(Default)]
struct AmplitudeStream {
    /// Audio of the current part.
    part: Vec<f32>,
    /// Samples heard since the stream started, and where the current part starts.
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

struct VecSource {
    samples: Arc<Vec<f32>>,
    frame_delay: Duration,
    cancelled: Option<Arc<AtomicBool>>,
    thread: Option<JoinHandle<()>>,
}

impl VecSource {
    fn new(samples: Vec<f32>, frame_delay: Duration) -> Self {
        Self {
            samples: Arc::new(samples),
            frame_delay,
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
        let delay = self.frame_delay;
        self.thread = Some(thread::spawn(move || {
            for (index, chunk) in samples.chunks(512).enumerate() {
                let t0 = StreamTime(producer.base_time.samples() + index as u64 * 512);
                let deadline = Instant::now() + delay;
                while Instant::now() < deadline {
                    if producer.cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    thread::sleep(Duration::from_micros(200));
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
        }));
        Ok(SourceInfo {
            mode: CaptureMode::System,
            label: "Test tones".into(),
            sample_rate: 16_000,
            channels: 1,
        })
    }
    fn stop(&mut self) {
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Release);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for VecSource {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A draft translator with seeded delays and failures. It answers `d<chars>` so each draft is
/// distinct text, and appends to whatever prefill it is given.
struct ScriptedDraft {
    state: u64,
    fail_percent: u64,
    max_delay_ms: u64,
    prefills: Arc<std::sync::Mutex<Vec<String>>>,
}

impl ScriptedDraft {
    fn new(seed: u64, fail_percent: u64, max_delay_ms: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            fail_percent,
            max_delay_ms,
            prefills: Arc::default(),
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

impl Translator for ScriptedDraft {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            streaming: false,
            glossary: false,
            context: false,
            prefill: true,
            max_input_chars: 300,
            pairs: vec![("zh".into(), "en".into())],
        }
    }
    fn warm_up(&mut self, _: &TranslationControl) -> Result<()> {
        Ok(())
    }
    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        _: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        self.prefills.lock().unwrap().push(request.prefill.into());
        let roll = self.next();
        if self.max_delay_ms > 0 {
            thread::sleep(Duration::from_millis(roll % (self.max_delay_ms + 1)));
        }
        if (roll >> 20) % 100 < self.fail_percent {
            return Err(Error::Translation {
                reason: FailReason::ServerUnavailable,
                message: "scripted failure".into(),
            });
        }
        // A growing English sentence, one word per two Chinese characters; with a prefill the
        // reply is only the continuation, as the real draft client returns it.
        let words: Vec<String> = (0..request.text.chars().count() / 2 + 3)
            .map(|i| format!("w{i}"))
            .collect();
        let done = request.prefill.split_whitespace().count();
        Ok(TranslationOut {
            text: words[done.min(words.len())..].join(" "),
            ..TranslationOut::default()
        })
    }
}

struct Run {
    events: Vec<PipelineEvent>,
}

fn run(samples: Vec<f32>, config: Config, draft: Option<ScriptedDraft>, final_delay: u32) -> Run {
    let bus = EventBus::default();
    let receiver = bus.subscribe(8192);
    let translator = FakeTranslator {
        delay_per_character: Duration::from_millis(u64::from(final_delay)),
        ..FakeTranslator::default()
    };
    let mut pipeline = Pipeline::start_with_options(
        config,
        Box::new(VecSource::new(samples, Duration::from_millis(1))),
        Box::<AmplitudeStream>::default(),
        Box::new(translator),
        bus,
        PipelineOptions {
            draft: draft.map(|d| Box::new(d) as Box<dyn Translator>),
            physical_cores: Some(8),
            ..PipelineOptions::default()
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !pipeline.is_finished() {
        assert!(Instant::now() < deadline, "the replay did not finish");
        thread::sleep(Duration::from_millis(5));
    }
    pipeline.wait().unwrap();
    Run {
        events: receiver.try_iter().collect(),
    }
}

fn config(mode: &str) -> Config {
    let mut config = Config::default();
    config.join.hold_max_chars = 0;
    config.latency.mode = mode.into();
    config.latency.step_down = false;
    config
}

/// The per-id ordering rules.
fn assert_ordering(events: &[PipelineEvent]) {
    #[derive(Default)]
    struct Line {
        asr_final: bool,
        dropped: bool,
        terminal: bool,
        absorbed: bool,
        drafts_after_final: u32,
        last_rev: u32,
    }
    let mut lines: BTreeMap<UtteranceId, Line> = BTreeMap::new();
    let mut seen_ids: BTreeSet<UtteranceId> = BTreeSet::new();
    for event in events {
        match event {
            PipelineEvent::SpeechStarted { id, .. } => {
                assert!(seen_ids.insert(*id), "repeated start {id:?}");
                lines.entry(*id).or_default();
            }
            PipelineEvent::AsrPartial { id, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(
                    !line.asr_final && !line.dropped,
                    "partial after the final: {id:?}"
                );
            }
            PipelineEvent::TranslationDraft { id, rev, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(
                    !line.terminal && !line.absorbed,
                    "draft after a terminal: {id:?}"
                );
                assert!(*rev > line.last_rev, "rev must increase: {id:?}");
                assert_eq!(
                    line.last_rev + 1,
                    *rev,
                    "rev counts published drafts: {id:?}"
                );
                line.last_rev = *rev;
                if line.asr_final {
                    line.drafts_after_final += 1;
                    assert!(
                        line.drafts_after_final <= 1,
                        "more than one draft after the final: {id:?}"
                    );
                }
            }
            PipelineEvent::AsrFinal { id, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(!line.asr_final && !line.dropped, "repeated final: {id:?}");
                line.asr_final = true;
            }
            PipelineEvent::Dropped { id, .. } => {
                let line = lines.entry(*id).or_default();
                assert!(!line.asr_final, "drop after the final: {id:?}");
                line.dropped = true;
                line.terminal = true;
            }
            PipelineEvent::Joined { absorbed, .. } => {
                for id in absorbed {
                    lines.entry(*id).or_default().absorbed = true;
                }
            }
            PipelineEvent::TranslationDelta { id, .. } => {
                assert!(!lines[id].terminal, "delta after a terminal: {id:?}");
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

#[test]
fn a_long_sentence_is_committed_into_clauses_that_add_up_to_the_whole_text() {
    let chars = sentence_chars();
    let run = run(speech(chars.len(), 1.0), config("light"), None, 0);
    assert_ordering(&run.events);
    let clauses = finals(&run.events);
    assert!(clauses.len() >= 2, "{clauses:?}");
    assert!(
        clauses.iter().any(|(_, _, cut)| *cut == CutReason::Commit),
        "no early commit: {clauses:?}"
    );
    let joined: String = clauses.iter().map(|(_, text, _)| text.as_str()).collect();
    assert_eq!(joined, SENTENCE);
    assert!(clauses.windows(2).all(|pair| pair[0].0 < pair[1].0));
    // The live Chinese line grew before the first final, and never after a clause's final.
    assert!(run
        .events
        .iter()
        .any(|event| matches!(event, PipelineEvent::AsrPartial { .. })));
    let stats_mode = run.events.iter().find_map(|event| match event {
        PipelineEvent::Stats(stats) => Some(stats.mode),
        _ => None,
    });
    assert!(stats_mode.is_none_or(|mode| mode == EffectiveMode::Light));
}

/// Off cuts at the sentence end and the pause, not at the early comma, and still shows the
/// Chinese while it is spoken.
#[test]
fn off_mode_cuts_at_sentence_ends_and_pauses_and_shows_live_text() {
    let chars = sentence_chars();
    let run = run(speech(chars.len(), 1.0), config("off"), None, 0);
    assert_ordering(&run.events);
    assert!(run
        .events
        .iter()
        .any(|event| matches!(event, PipelineEvent::AsrPartial { .. })));
    assert!(!run
        .events
        .iter()
        .any(|event| matches!(event, PipelineEvent::TranslationDraft { .. })));
    let clauses = finals(&run.events);
    let texts: Vec<_> = clauses.iter().map(|(_, text, _)| text.as_str()).collect();
    let (sentence, rest) = SENTENCE.split_at(SENTENCE.find('。').unwrap() + '。'.len_utf8());
    assert_eq!(texts, [sentence, rest]);
    let cuts: Vec<_> = clauses.iter().map(|(_, _, cut)| *cut).collect();
    assert_eq!(cuts, [CutReason::Commit, CutReason::Pause]);
}

#[test]
fn continuous_mode_publishes_drafts_that_continue_each_other_and_stop_at_the_final() {
    let chars = sentence_chars();
    let draft = ScriptedDraft::new(7, 0, 4);
    let prefills = draft.prefills.clone();
    let run = run(speech(chars.len(), 1.0), config("auto"), Some(draft), 10);
    assert_ordering(&run.events);
    let drafts: Vec<_> = run
        .events
        .iter()
        .filter_map(|event| match event {
            PipelineEvent::TranslationDraft { id, rev, text, .. } => {
                Some((*id, *rev, text.clone()))
            }
            _ => None,
        })
        .collect();
    assert!(drafts.len() >= 3, "{drafts:?}");
    assert!(drafts.iter().any(|(_, rev, _)| *rev >= 2));
    // Later drafts of one clause are prefilled with the earlier English.
    assert!(
        prefills.lock().unwrap().iter().any(|p| !p.is_empty()),
        "no prefilled request"
    );
    for (_, _, text) in &drafts {
        assert!(!text.contains("  "), "double space in {text:?}");
    }
}

#[test]
fn many_seeds_keep_every_ordering_rule_with_random_delays_and_failures() {
    let chars = sentence_chars();
    for seed in 1..=10_u64 {
        let mut samples = Vec::new();
        for round in 0..3 {
            samples.extend(speech(
                chars.len() - (seed as usize + round) % 6,
                0.5 + 0.1 * round as f32,
            ));
        }
        let draft = ScriptedDraft::new(seed, 30, 8);
        let run = run(
            samples,
            config("continuous"),
            Some(draft),
            (seed % 4) as u32 * 3,
        );
        assert_ordering(&run.events);
        assert!(!finals(&run.events).is_empty(), "seed {seed}");
    }
}

#[test]
fn mode_resolution_follows_the_decisions() {
    use lt_core::{pipeline::resolve_mode, types::ModeReason};
    let mut latency = lt_core::config::LatencyConfig::default();
    let state = |latency: &lt_core::config::LatencyConfig, draft, cores| {
        let resolved = resolve_mode(latency, draft, cores);
        (resolved.mode, resolved.reason)
    };
    assert_eq!(
        state(&latency, true, 8),
        (EffectiveMode::Continuous, Some(ModeReason::Auto))
    );
    assert_eq!(
        state(&latency, true, 4),
        (EffectiveMode::Light, Some(ModeReason::Auto))
    );
    assert_eq!(
        state(&latency, false, 16),
        (EffectiveMode::Light, Some(ModeReason::Auto))
    );
    latency.mode = "continuous".into();
    assert_eq!(
        state(&latency, false, 2),
        (EffectiveMode::Light, Some(ModeReason::DraftUnavailable))
    );
    assert_eq!(
        state(&latency, true, 2),
        (EffectiveMode::Continuous, Some(ModeReason::User))
    );
    latency.mode = "light".into();
    assert_eq!(
        state(&latency, true, 16),
        (EffectiveMode::Light, Some(ModeReason::User))
    );
    latency.mode = "off".into();
    assert_eq!(
        state(&latency, true, 16),
        (EffectiveMode::Off, Some(ModeReason::User))
    );
}
