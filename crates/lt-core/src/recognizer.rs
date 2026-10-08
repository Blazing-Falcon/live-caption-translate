//! The recognizer: re-decodes the open clause as audio
//! arrives, runs the commit rule, and produces the clause transcripts the scheduler consumes.
//!
//! `Recognizer` is a plain state machine driven by `RecognizerInput`. The `lt-asr` thread in
//! `pipeline.rs` only moves messages in and out, so everything here is testable without threads.

use crate::{
    commit::{cut_at, decide, is_word_token, join_tokens, CommitSettings, Decision},
    config::Config,
    engines::{SegmentAsr, TokenTranscript},
    error::Result,
    events::DropReason,
    text::{classify_with_lang, clean, drop_reason},
    types::{
        CutReason, Segment, StageTiming, StreamTime, TextClass, Transcript, UtteranceId,
        SAMPLE_RATE,
    },
};
use std::time::Instant;

/// What the frontend tells the recognizer, in order.
#[derive(Clone, Debug)]
pub enum RecognizerInput {
    /// A clause opened. `start` equals the `Segment.start` it will close with.
    Open {
        id: UtteranceId,
        start: StreamTime,
        after_commit: bool,
    },
    /// Contiguous audio of the open clause, in order, beginning at `Open.start`.
    Audio { id: UtteranceId, samples: Vec<f32> },
    /// The clause closed; the segment, unchanged.
    Closed(Segment),
}

/// The recognizer asks the frontend to cut the open clause `id` at `at`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommitCut {
    pub id: UtteranceId,
    pub at: StreamTime,
}

#[derive(Debug)]
pub enum AsrMessage {
    Transcript(Transcript),
    Dropped(UtteranceId, DropReason),
    Failed(UtteranceId, String),
    /// Live text of the open clause.
    Partial {
        id: UtteranceId,
        text: String,
        class: TextClass,
        /// End of the decoded audio.
        end: StreamTime,
    },
}

impl AsrMessage {
    pub fn id(&self) -> UtteranceId {
        match self {
            Self::Transcript(transcript) => transcript.id,
            Self::Dropped(id, _) | Self::Failed(id, _) | Self::Partial { id, .. } => *id,
        }
    }
}

#[derive(Debug)]
pub enum Output {
    Message(AsrMessage),
    Cut(CommitCut),
}

struct PendingCut {
    at: StreamTime,
    prefix: String,
    tail: String,
    asr_ms: u32,
    lang: Option<String>,
    audio_end: StreamTime,
}

struct Clause {
    id: UtteranceId,
    start: StreamTime,
    /// Audio before `start` that the decoder may hear (after a commit only).
    context: Vec<f32>,
    samples: Vec<f32>,
    last_decode_len: Option<usize>,
    prev_text: Option<String>,
    pending: Option<PendingCut>,
    last_partial: String,
}

/// The first text of the clause that follows a commit.
struct Tail {
    text: String,
    lang: Option<String>,
    end: StreamTime,
}

pub struct Recognizer {
    config: Config,
    commit: CommitSettings,
    min_open: usize,
    interval: usize,
    draft_context: usize,
    final_context: usize,
    clause: Option<Clause>,
    carry: Vec<f32>,
    tail: Option<Tail>,
}

fn samples_for(seconds: f32) -> usize {
    (f64::from(seconds) * SAMPLE_RATE as f64).round() as usize
}

fn millis_rounded(seconds: f64) -> StreamTime {
    StreamTime::from_millis((seconds * 1000.0).round().max(0.0) as u64)
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// `context ++ samples`, keeping the last `limit` samples.
fn trimmed_context(context: &[f32], samples: &[f32], limit: usize) -> Vec<f32> {
    let total = context.len() + samples.len();
    let skip = total.saturating_sub(limit);
    context
        .iter()
        .chain(samples.iter())
        .skip(skip)
        .copied()
        .collect()
}

impl Recognizer {
    pub fn new(config: &Config) -> Self {
        let latency = &config.latency;
        Self {
            commit: CommitSettings::from(latency),
            min_open: samples_for(latency.min_open_s),
            interval: samples_for(latency.decode_interval_s).max(1),
            draft_context: samples_for(latency.draft_context_s),
            final_context: samples_for(latency.final_context_s),
            config: config.clone(),
            clause: None,
            carry: Vec::new(),
            tail: None,
        }
    }

    pub fn handle(
        &mut self,
        asr: &mut dyn SegmentAsr,
        input: RecognizerInput,
        now: StreamTime,
        out: &mut Vec<Output>,
    ) {
        match input {
            RecognizerInput::Open {
                id,
                start,
                after_commit,
            } => self.open(id, start, after_commit, out),
            RecognizerInput::Audio { id, samples } => {
                if let Some(clause) = self.clause.as_mut().filter(|clause| clause.id == id) {
                    clause.samples.extend_from_slice(&samples);
                    self.maybe_decode(asr, out);
                }
            }
            RecognizerInput::Closed(segment) => self.close(asr, segment, now, out),
        }
    }

    fn open(
        &mut self,
        id: UtteranceId,
        start: StreamTime,
        after_commit: bool,
        out: &mut Vec<Output>,
    ) {
        let context = if after_commit {
            std::mem::take(&mut self.carry)
        } else {
            self.carry.clear();
            Vec::new()
        };
        let tail = self.tail.take().filter(|_| after_commit);
        self.clause = Some(Clause {
            id,
            start,
            context,
            samples: Vec::new(),
            last_decode_len: None,
            prev_text: None,
            pending: None,
            last_partial: String::new(),
        });
        // The live line continues at once from the text after the cut.
        if let Some(tail) = tail {
            self.send_partial(&tail.text, tail.lang.as_deref(), tail.end, out);
        }
    }

    fn send_partial(
        &mut self,
        text: &str,
        lang: Option<&str>,
        end: StreamTime,
        out: &mut Vec<Output>,
    ) {
        let Some(clause) = self.clause.as_mut() else {
            return;
        };
        let cleaned = clean(text, &self.config.filter.fillers);
        if cleaned.is_empty() || cleaned == clause.last_partial {
            return;
        }
        let lang = crate::pipeline::normalized_language(lang);
        let Some(class) = classify_with_lang(&cleaned, lang.as_deref()) else {
            return;
        };
        clause.last_partial.clone_from(&cleaned);
        out.push(Output::Message(AsrMessage::Partial {
            id: clause.id,
            text: cleaned,
            class,
            end,
        }));
    }

    fn maybe_decode(&mut self, asr: &mut dyn SegmentAsr, out: &mut Vec<Output>) {
        let Some(clause) = self.clause.as_ref() else {
            return;
        };
        let length = clause.samples.len();
        if clause.pending.is_some()
            || length < self.min_open
            || clause
                .last_decode_len
                .is_some_and(|last| length < last + self.interval)
        {
            return;
        }
        let Some(window_asr) = asr.window() else {
            return;
        };
        let context_len = self.draft_context.min(clause.context.len());
        let mut window = Vec::with_capacity(context_len + length);
        window.extend_from_slice(&clause.context[clause.context.len() - context_len..]);
        window.extend_from_slice(&clause.samples);
        let started = Instant::now();
        let decoded = window_asr.decode_window(&window);
        let asr_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
        let Some(clause) = self.clause.as_mut() else {
            return;
        };
        clause.last_decode_len = Some(length);
        let transcript = match decoded {
            Ok(transcript) => transcript,
            Err(error) => {
                tracing::debug!(id = clause.id.0, %error, "window decode failed");
                return;
            }
        };
        let (tokens, times) = shift_tokens(&transcript, context_len);
        let end = StreamTime(clause.start.0 + length as u64);
        let text = if tokens.is_empty() {
            transcript.text.trim().to_owned()
        } else {
            join_tokens(&tokens)
        };
        let decision = if !tokens.is_empty() && times.len() == tokens.len() {
            decide(&tokens, &times, clause.prev_text.as_deref(), &self.commit)
        } else {
            Decision::None
        };
        match decision {
            Decision::Commit { k } | Decision::Cap { k } => {
                let at =
                    millis_rounded(cut_at(clause.start.seconds(), times[k + 1], end.seconds()));
                clause.pending = Some(PendingCut {
                    at,
                    prefix: join_tokens(&tokens[..=k]),
                    tail: join_tokens(&tokens[k + 1..]),
                    asr_ms,
                    lang: transcript.lang_tag.clone(),
                    audio_end: end,
                });
                out.push(Output::Cut(CommitCut { id: clause.id, at }));
            }
            Decision::None | Decision::WaitStable { .. } => {
                clause.prev_text = Some(text.clone());
                self.send_partial(&text, transcript.lang_tag.as_deref(), end, out);
            }
        }
    }

    fn close(
        &mut self,
        asr: &mut dyn SegmentAsr,
        segment: Segment,
        now: StreamTime,
        out: &mut Vec<Output>,
    ) {
        let clause = self.clause.take().filter(|clause| clause.id == segment.id);
        let committed = segment.cut_reason == CutReason::Commit;
        if let Some(clause) = &clause {
            if let Some(pending) = clause.pending.as_ref().filter(|p| p.at == segment.end) {
                if committed {
                    let transcript = Transcript {
                        id: segment.id,
                        text: pending.prefix.clone(),
                        lang_tag: pending.lang.clone(),
                        class: TextClass::Chinese,
                        event: None,
                        timing: StageTiming::default(),
                        absorbed: Vec::new(),
                        cut: CutReason::Commit,
                    };
                    out.push(Output::Message(finalize(
                        transcript,
                        &segment,
                        pending.asr_ms,
                        now,
                        &self.config,
                    )));
                    self.carry =
                        trimmed_context(&clause.context, &segment.samples, self.final_context);
                    self.tail = Some(Tail {
                        text: pending.tail.clone(),
                        lang: pending.lang.clone(),
                        end: pending.audio_end,
                    });
                    return;
                }
            }
        }
        // Any other close discards a pending cut and decodes the clause for its final text.
        self.tail = None;
        let started = Instant::now();
        let result = match clause.as_ref().filter(|clause| !clause.context.is_empty()) {
            Some(clause) if asr.window().is_some() => {
                self.final_with_context(asr, clause, &segment)
            }
            _ => asr.transcribe(&segment),
        };
        let asr_ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
        self.carry = match (&clause, committed) {
            (Some(clause), true) => {
                trimmed_context(&clause.context, &segment.samples, self.final_context)
            }
            _ => Vec::new(),
        };
        out.push(Output::Message(match result {
            Ok(transcript) => finalize(transcript, &segment, asr_ms, now, &self.config),
            Err(error) => AsrMessage::Failed(segment.id, error.to_string()),
        }));
    }

    /// The final decode of a clause that follows a commit hears the whole available context:
    /// a short window can start mid-word (juan's clip: 麒麟9050pro became 709050pro).
    fn final_with_context(
        &self,
        asr: &mut dyn SegmentAsr,
        clause: &Clause,
        segment: &Segment,
    ) -> Result<Transcript> {
        let context_len = self.final_context.min(clause.context.len());
        let mut window = Vec::with_capacity(context_len + segment.samples.len());
        window.extend_from_slice(&clause.context[clause.context.len() - context_len..]);
        window.extend_from_slice(&segment.samples);
        let Some(window_asr) = asr.window() else {
            return asr.transcribe(segment);
        };
        let decoded = window_asr.decode_window(&window)?;
        let (tokens, _) = shift_tokens(&decoded, context_len);
        let text = if tokens.is_empty() && context_len == 0 {
            decoded.text.trim().to_owned()
        } else {
            join_tokens(&tokens)
        };
        Ok(Transcript {
            id: segment.id,
            text,
            lang_tag: decoded.lang_tag,
            class: TextClass::Chinese,
            event: decoded.event,
            timing: StageTiming::default(),
            absorbed: Vec::new(),
            cut: segment.cut_reason,
        })
    }
}

/// Tokens and start times relative to the clause start. With context, the context's own
/// tokens (time below -0.03 s) and any non-word tokens left at the front are dropped.
fn shift_tokens(transcript: &TokenTranscript, context_len: usize) -> (Vec<String>, Vec<f64>) {
    let shift = context_len as f64 / SAMPLE_RATE as f64;
    let mut tokens = Vec::with_capacity(transcript.tokens.len());
    let mut times = Vec::with_capacity(transcript.tokens.len());
    for (token, time) in transcript.tokens.iter().zip(&transcript.timestamps) {
        let relative = round3(f64::from(*time) - shift);
        if context_len > 0 && relative < -0.03 {
            continue;
        }
        tokens.push(token.clone());
        times.push(relative);
    }
    if context_len > 0 {
        let lead = tokens.iter().take_while(|t| !is_word_token(t)).count();
        tokens.drain(..lead);
        times.drain(..lead);
    }
    if transcript.timestamps.len() != transcript.tokens.len() {
        // Without one time per token the commit rule cannot place a cut.
        times.clear();
    }
    (tokens, times)
}

/// Apply the segment path's cleaning, language normalization, classification and drop rules to a clause
/// transcript and stamp its timing from the segment.
pub(crate) fn finalize(
    mut transcript: Transcript,
    segment: &Segment,
    asr_ms: u32,
    now: StreamTime,
    config: &Config,
) -> AsrMessage {
    transcript.id = segment.id;
    transcript.timing.start = segment.start;
    transcript.timing.end = segment.end;
    transcript.timing.asr_ms = asr_ms;
    transcript.timing.asr_done_ms = now.millis();
    transcript.cut = segment.cut_reason;
    transcript.text = clean(&transcript.text, &config.filter.fillers);
    transcript.lang_tag = crate::pipeline::normalized_language(transcript.lang_tag.as_deref());
    let class = classify_with_lang(&transcript.text, transcript.lang_tag.as_deref());
    let Some(class) = class else {
        return AsrMessage::Dropped(segment.id, DropReason::Empty);
    };
    transcript.class = class;
    if class == TextClass::Other && transcript.lang_tag.is_none() {
        transcript.lang_tag = crate::pipeline::inferred_other_language(&transcript.text);
    }
    match drop_reason(
        &transcript,
        segment.end.saturating_sub(segment.start).seconds(),
        &config.filter,
    ) {
        Some(reason) => AsrMessage::Dropped(segment.id, reason),
        None => AsrMessage::Transcript(transcript),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::WindowAsr;
    use serde_json::Value;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    /// Replays the recorded decodes of one trace, in order.
    struct TraceAsr {
        decodes: Arc<Mutex<VecDeque<Value>>>,
        finals: Arc<Mutex<VecDeque<Value>>>,
        window_lengths: Arc<Mutex<Vec<(usize, usize)>>>,
    }

    impl WindowAsr for TraceAsr {
        fn decode_window(&mut self, samples: &[f32]) -> Result<TokenTranscript> {
            let recorded = self
                .decodes
                .lock()
                .unwrap()
                .pop_front()
                .or_else(|| self.finals.lock().unwrap().pop_front())
                .expect("a recorded decode for every window");
            let context = (recorded["context_s"].as_f64().unwrap() * 16_000.0).round() as usize;
            self.window_lengths
                .lock()
                .unwrap()
                .push((samples.len(), context));
            let shift = recorded["context_s"].as_f64().unwrap();
            let tokens: Vec<String> = recorded["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            let timestamps: Vec<f32> = recorded["timestamps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| (v.as_f64().unwrap() + shift) as f32)
                .collect();
            Ok(TokenTranscript {
                text: tokens.concat().trim().to_owned(),
                tokens,
                timestamps,
                lang_tag: Some("zh".into()),
                event: None,
            })
        }
    }

    struct TraceEngine {
        window: TraceAsr,
        final_text: Arc<Mutex<VecDeque<String>>>,
    }

    impl SegmentAsr for TraceEngine {
        fn transcribe(&mut self, segment: &Segment) -> Result<Transcript> {
            let text = self
                .final_text
                .lock()
                .unwrap()
                .pop_front()
                .expect("a recorded final text");
            Ok(Transcript {
                id: segment.id,
                text,
                lang_tag: Some("zh".into()),
                class: TextClass::Chinese,
                event: None,
                timing: StageTiming::default(),
                absorbed: Vec::new(),
                cut: segment.cut_reason,
            })
        }
        fn window(&mut self) -> Option<&mut dyn WindowAsr> {
            Some(&mut self.window)
        }
    }

    fn frame_samples(seconds: f64) -> usize {
        // decode instants in the trace are 32 ms frame ends
        ((seconds / 0.032).round() as usize) * 512
    }

    fn reason(name: &str) -> CutReason {
        match name {
            "pause" => CutReason::Pause,
            "soft cut" => CutReason::SoftCut,
            "hard cut" => CutReason::HardCut,
            "end" => CutReason::End,
            other => panic!("unknown reason {other}"),
        }
    }

    struct Replay {
        transcripts: Vec<Transcript>,
        partials: Vec<(UtteranceId, String)>,
        windows: Vec<(usize, usize)>,
        discarded_cuts: usize,
    }

    fn config_for(trace: &Value) -> Config {
        let mut config = Config::default();
        let settings = &trace["config"];
        let get = |key: &str| settings[key].as_str().unwrap().to_owned();
        config.latency.comma_min_tokens = get("comma_min").parse().unwrap();
        config.latency.stability = get("stable") == "1";
        config.latency.cap_tokens = get("cap").parse().unwrap();
        config.latency.split_long = true;
        config
    }

    fn replay(json: &str, nothing_pending_check: bool) -> (Replay, Value) {
        let trace: Value = serde_json::from_str(json).unwrap();
        let config = config_for(&trace);
        let mut recognizer = Recognizer::new(&config);
        let decodes = Arc::new(Mutex::new(VecDeque::new()));
        let finals = Arc::new(Mutex::new(VecDeque::new()));
        let final_text = Arc::new(Mutex::new(VecDeque::new()));
        let window_lengths = Arc::new(Mutex::new(Vec::new()));
        let mut engine = TraceEngine {
            window: TraceAsr {
                decodes: decodes.clone(),
                finals: finals.clone(),
                window_lengths: window_lengths.clone(),
            },
            final_text: final_text.clone(),
        };
        let mut replayed = Replay {
            transcripts: Vec::new(),
            partials: Vec::new(),
            windows: Vec::new(),
            discarded_cuts: 0,
        };
        let mut next_id = 0_u64;
        for piece in trace["pieces"].as_array().unwrap() {
            let mut id = UtteranceId(next_id);
            next_id += 1;
            let piece_start = millis_rounded(piece["start"].as_f64().unwrap());
            let mut clause_start = piece_start;
            let mut after_commit = false;
            let mut out = Vec::new();
            recognizer.handle(
                &mut engine,
                RecognizerInput::Open {
                    id,
                    start: clause_start,
                    after_commit,
                },
                clause_start,
                &mut out,
            );
            let decode_list = piece["decodes"].as_array().unwrap();
            // Queue exactly the decodes of this piece; the recognizer must consume all of them.
            decodes.lock().unwrap().clear();
            for decode in decode_list {
                decodes.lock().unwrap().push_back(decode.clone());
            }
            finals
                .lock()
                .unwrap()
                .push_back(piece["final_decode"].clone());
            final_text
                .lock()
                .unwrap()
                .push_back(piece["final_decode"]["text"].as_str().unwrap().to_owned());
            let mut contextual_final = false;
            for decode in decode_list {
                let target = frame_samples(decode["audio_end"].as_f64().unwrap());
                let wanted = target - clause_start.0 as usize;
                out.clear();
                recognizer.handle(
                    &mut engine,
                    RecognizerInput::Audio {
                        id,
                        samples: vec![0.0; wanted - clause_audio_len(&recognizer)],
                    },
                    StreamTime(target as u64),
                    &mut out,
                );
                for output in out.drain(..) {
                    match output {
                        Output::Message(AsrMessage::Partial { id, text, .. }) => {
                            replayed.partials.push((id, text));
                        }
                        Output::Message(other) => panic!("unexpected {other:?}"),
                        Output::Cut(cut) => {
                            assert_eq!(cut.id, id);
                            let segment = Segment {
                                id,
                                start: clause_start,
                                end: cut.at,
                                samples: vec![0.0; (cut.at.0 - clause_start.0) as usize].into(),
                                cut_reason: CutReason::Commit,
                            };
                            let mut closed = Vec::new();
                            recognizer.handle(
                                &mut engine,
                                RecognizerInput::Closed(segment),
                                cut.at,
                                &mut closed,
                            );
                            for output in closed {
                                if let Output::Message(AsrMessage::Transcript(t)) = output {
                                    replayed.transcripts.push(t);
                                }
                            }
                            id = UtteranceId(next_id);
                            next_id += 1;
                            clause_start = cut.at;
                            after_commit = true;
                            contextual_final = true;
                            let mut opened = Vec::new();
                            recognizer.handle(
                                &mut engine,
                                RecognizerInput::Open {
                                    id,
                                    start: cut.at,
                                    after_commit,
                                },
                                cut.at,
                                &mut opened,
                            );
                            for output in opened {
                                if let Output::Message(AsrMessage::Partial { id, text, .. }) =
                                    output
                                {
                                    replayed.partials.push((id, text));
                                }
                            }
                            // The audio since the cut reaches the new clause with the next
                            // chunk, as the frontend's frame-sized forwarding does.
                        }
                    }
                }
            }
            assert!(
                decodes.lock().unwrap().is_empty(),
                "the recognizer consumed every recorded decode of the piece"
            );
            // Close the piece with its recorded reason.
            let end = millis_rounded(piece["end"].as_f64().unwrap());
            // The closing frame belongs to the segment; it is not forwarded as open-clause audio
            // (the reference decodes the closing frame's audio only as the final).
            let _ = nothing_pending_check;
            let segment = Segment {
                id,
                start: clause_start,
                end,
                samples: vec![0.0; (end.0 - clause_start.0) as usize].into(),
                cut_reason: reason(piece["reason"].as_str().unwrap()),
            };
            let mut closed = Vec::new();
            recognizer.handle(
                &mut engine,
                RecognizerInput::Closed(segment),
                end,
                &mut closed,
            );
            let _ = contextual_final;
            for output in closed {
                match output {
                    Output::Message(AsrMessage::Transcript(t)) => replayed.transcripts.push(t),
                    Output::Message(AsrMessage::Dropped(..)) => {}
                    other => panic!("unexpected close output {other:?}"),
                }
            }
            finals.lock().unwrap().clear();
            final_text.lock().unwrap().clear();
        }
        replayed.windows = window_lengths.lock().unwrap().clone();
        replayed.discarded_cuts = 0;
        (replayed, trace)
    }

    /// Length of the open clause's audio, as the recognizer sees it.
    fn clause_audio_len(recognizer: &Recognizer) -> usize {
        recognizer
            .clause
            .as_ref()
            .map_or(0, |clause| clause.samples.len())
    }

    fn assert_trace_clauses(json: &str) {
        let (replayed, trace) = replay(json, true);
        let expected: Vec<&Value> = trace["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|piece| piece["clauses"].as_array().unwrap())
            .collect();
        assert_eq!(
            replayed.transcripts.len(),
            expected.len(),
            "clause count: {:?}",
            replayed
                .transcripts
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
        );
        for (got, want) in replayed.transcripts.iter().zip(&expected) {
            assert_eq!(
                got.text,
                clean(
                    want["text"].as_str().unwrap(),
                    &Config::default().filter.fillers
                ),
                "kind {}",
                want["kind"]
            );
            let start = want["start"].as_f64().unwrap();
            let end = want["end"].as_f64().unwrap();
            assert!(
                (got.timing.start.seconds() - start).abs() <= 0.001,
                "start {} vs {start}",
                got.timing.start.seconds()
            );
            assert!(
                (got.timing.end.seconds() - end).abs() <= 0.001,
                "end {} vs {end}",
                got.timing.end.seconds()
            );
            let kind = want["kind"].as_str().unwrap();
            assert_eq!(got.cut == CutReason::Commit, kind != "close", "kind {kind}");
        }
    }

    #[test]
    fn juan_trace_clauses_are_reproduced() {
        assert_trace_clauses(include_str!("../tests/fixtures/v2/commit-trace-juan.json"));
    }

    #[test]
    fn juan_trace_with_the_cap_is_reproduced() {
        assert_trace_clauses(include_str!(
            "../tests/fixtures/v2/commit-trace-juan-cap20.json"
        ));
    }

    #[test]
    fn conversation_trace_is_reproduced() {
        assert_trace_clauses(include_str!(
            "../tests/fixtures/v2/commit-trace-conv60.json"
        ));
    }

    #[test]
    fn keynote_trace_is_reproduced() {
        assert_trace_clauses(include_str!(
            "../tests/fixtures/v2/commit-trace-keynote60.json"
        ));
    }

    #[test]
    fn final_decodes_use_the_recorded_context() {
        let (replayed, _) = replay(
            include_str!("../tests/fixtures/v2/commit-trace-juan.json"),
            true,
        );
        assert!(!replayed.windows.is_empty());
        for (window, context) in &replayed.windows {
            assert!(window >= context, "window {window} holds context {context}");
        }
        // A window decoded with a 10 s context exists: the final decode after a commit.
        assert!(replayed
            .windows
            .iter()
            .any(|(_, context)| *context > 24_000));
    }

    #[test]
    fn partials_continue_straight_after_a_commit() {
        let (replayed, trace) = replay(
            include_str!("../tests/fixtures/v2/commit-trace-juan.json"),
            true,
        );
        let tails: Vec<String> = trace["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|piece| piece["decodes"].as_array().unwrap())
            .filter(|decode| decode["decision"] == "commit")
            .map(|decode| clean(decode["tail"].as_str().unwrap(), &[]))
            .filter(|tail| !tail.is_empty())
            .collect();
        assert!(!tails.is_empty());
        for tail in tails {
            assert!(
                replayed.partials.iter().any(|(_, text)| *text == tail),
                "no partial carried the tail {tail:?}"
            );
        }
    }

    #[test]
    fn a_close_that_does_not_match_the_pending_cut_discards_it() {
        let mut config = Config::default();
        config.latency.stability = false;
        let mut recognizer = Recognizer::new(&config);
        let decodes = Arc::new(Mutex::new(VecDeque::new()));
        let finals = Arc::new(Mutex::new(VecDeque::new()));
        let mut engine = TraceEngine {
            window: TraceAsr {
                decodes: decodes.clone(),
                finals,
                window_lengths: Arc::new(Mutex::new(Vec::new())),
            },
            final_text: Arc::new(Mutex::new(VecDeque::from(["你好吗我很好".to_owned()]))),
        };
        let tokens: Vec<&str> = vec!["你", "好", "吗", "。", "我", "很", "好"];
        let times: Vec<f64> = (0..tokens.len()).map(|i| i as f64 * 0.2).collect();
        decodes.lock().unwrap().push_back(serde_json::json!({
            "context_s": 0.0, "tokens": tokens, "timestamps": times,
        }));
        let id = UtteranceId(0);
        let mut out = Vec::new();
        recognizer.handle(
            &mut engine,
            RecognizerInput::Open {
                id,
                start: StreamTime::ZERO,
                after_commit: false,
            },
            StreamTime::ZERO,
            &mut out,
        );
        recognizer.handle(
            &mut engine,
            RecognizerInput::Audio {
                id,
                samples: vec![0.0; 16_000],
            },
            StreamTime(16_000),
            &mut out,
        );
        assert!(matches!(out.last(), Some(Output::Cut(_))), "{out:?}");
        // The piece ended (pause) before the frontend saw the request.
        out.clear();
        recognizer.handle(
            &mut engine,
            RecognizerInput::Closed(Segment {
                id,
                start: StreamTime::ZERO,
                end: StreamTime(16_000),
                samples: vec![0.0; 16_000].into(),
                cut_reason: CutReason::Pause,
            }),
            StreamTime(16_000),
            &mut out,
        );
        match out.as_slice() {
            [Output::Message(AsrMessage::Transcript(t))] => {
                assert_eq!(t.text, "你好吗我很好");
                assert_eq!(t.cut, CutReason::Pause);
            }
            other => panic!("{other:?}"),
        }
        assert!(recognizer.carry.is_empty());
    }
}
