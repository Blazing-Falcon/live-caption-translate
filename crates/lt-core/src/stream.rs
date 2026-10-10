//! Clauses from a streaming recognizer that hears all audio continuously.
//!
//! The engine's text only grows. A clause opens at its first word and ends at a commit
//! (punctuation or the wait cap, the same rule as the windowed recognizer), at the engine's
//! pause rule, or at a break in the audio. Each clause gets an id, start and close notices and
//! its audio, so the scheduler, joiner and second pass work as with the segment builder.
//!
//! `StreamRecognizer` is a plain state machine; the `lt-asr` thread only moves its inputs and
//! outputs.

use crate::{
    commit::{cut_at, decide, is_word_token, join_tokens, CommitSettings, Decision},
    config::Config,
    engines::{mean_log_prob, StreamResult, StreamingAsr},
    recognizer::{finalize, AsrMessage},
    text::{classify_with_lang, clean},
    types::{CutReason, Segment, StageTiming, StreamTime, TextClass, Transcript, UtteranceId},
};
use std::{collections::VecDeque, time::Instant};

/// Audio kept before the first word of a clause, like the segment builder's pre-roll.
const PRE_ROLL_S: f64 = 0.3;
/// A clause that ends at a pause or break keeps this much audio after its last token start,
/// so the second pass hears the last word.
const LAST_WORD_S: f64 = 0.6;
/// Audio kept while no clause is open: a token can appear up to a chunk plus the encoder's
/// look-ahead after it was spoken.
const IDLE_KEEP_S: f64 = 3.0;
/// The longest clause audio kept for the second pass.
const MAX_CLAUSE_S: f64 = 30.0;

/// What the frontend sends in streaming mode.
#[derive(Clone, Debug)]
pub enum StreamInput {
    /// Contiguous audio starting at `at`.
    Audio { at: StreamTime, samples: Vec<f32> },
    /// The audio stops or jumps (pause, device change, end of file).
    Break { at: StreamTime, reason: CutReason },
}

#[derive(Debug)]
pub enum StreamOutput {
    Started(UtteranceId, StreamTime),
    Closed(UtteranceId, StreamTime, CutReason),
    Message(AsrMessage),
}

struct Clause {
    id: UtteranceId,
    start: StreamTime,
    last_partial: String,
}

pub struct StreamRecognizer {
    config: Config,
    commit: CommitSettings,
    partials: bool,
    next_id: u64,
    /// Stream time of engine time 0; `None` until audio arrives after a break.
    origin: Option<StreamTime>,
    /// Audio from `audio_start` to the newest sample.
    audio: VecDeque<f32>,
    audio_start: StreamTime,
    clause: Option<Clause>,
    /// Tokens of the engine's current part that belong to finished clauses.
    consumed: usize,
}

impl StreamRecognizer {
    /// `partials`: publish the open clause's text (not in Off mode).
    pub fn new(config: &Config, partials: bool) -> Self {
        Self {
            commit: CommitSettings {
                // Streaming tokens never change, so a commit needs no second look.
                stability: false,
                ..CommitSettings::from(&config.latency)
            },
            config: config.clone(),
            partials,
            next_id: 0,
            origin: None,
            audio: VecDeque::new(),
            audio_start: StreamTime::ZERO,
            clause: None,
            consumed: 0,
        }
    }

    pub fn handle(
        &mut self,
        engine: &mut dyn StreamingAsr,
        input: StreamInput,
        out: &mut Vec<StreamOutput>,
    ) {
        match input {
            StreamInput::Audio { at, samples } => {
                if self.origin.is_none() {
                    self.origin = Some(at);
                    self.audio.clear();
                    self.audio_start = at;
                }
                self.audio.extend(&samples);
                let started = Instant::now();
                let result = engine.accept(&samples).and_then(|()| engine.result());
                let asr_ms = elapsed_ms(started);
                match result {
                    Ok(result) => {
                        self.process(&result, asr_ms, false, CutReason::Pause, out);
                        if engine.is_endpoint() {
                            self.process(&result, asr_ms, true, CutReason::Pause, out);
                            engine.reset();
                            self.consumed = 0;
                        }
                    }
                    Err(error) => self.fail(&error.to_string(), out),
                }
                self.trim();
            }
            StreamInput::Break { reason, .. } => {
                let started = Instant::now();
                match engine.finish() {
                    Ok(result) => self.process(&result, elapsed_ms(started), true, reason, out),
                    Err(error) => self.fail(&error.to_string(), out),
                }
                self.origin = None;
                self.consumed = 0;
                self.audio.clear();
            }
        }
    }

    fn end_of_audio(&self) -> StreamTime {
        StreamTime(self.audio_start.0 + self.audio.len() as u64)
    }

    /// Commit what the rule allows; with `close`, end the open clause with the rest.
    fn process(
        &mut self,
        result: &StreamResult,
        asr_ms: u32,
        close: bool,
        reason: CutReason,
        out: &mut Vec<StreamOutput>,
    ) {
        let Some(origin) = self.origin else {
            return;
        };
        let tokens = &result.tokens;
        let count = tokens.tokens.len().min(tokens.timestamps.len());
        let base = origin.seconds() + result.start_s;
        let at = |index: usize| base + f64::from(tokens.timestamps[index]);
        let audio_end = self.end_of_audio();
        loop {
            let rest = self.consumed..count;
            if rest.is_empty() {
                break;
            }
            let Some(first_word) = rest.clone().find(|&i| is_word_token(&tokens.tokens[i])) else {
                break;
            };
            let clause_start = match &self.clause {
                Some(clause) => clause.start,
                None => {
                    // Clause audio never overlaps: earlier audio was dropped at the last close.
                    let start = StreamTime::from_seconds((at(first_word) - PRE_ROLL_S).max(0.0))
                        .max(self.audio_start);
                    self.open(start, out);
                    start
                }
            }
            .seconds();
            let words: Vec<String> = tokens.tokens[rest.clone()].to_vec();
            let times: Vec<f64> = rest.clone().map(|i| at(i) - clause_start).collect();
            match decide(&words, &times, None, &self.commit) {
                Decision::Commit { k } | Decision::Cap { k } => {
                    let cut = StreamTime::from_seconds(cut_at(
                        clause_start,
                        times[k + 1],
                        audio_end.seconds(),
                    ));
                    let range = self.consumed..self.consumed + k + 1;
                    self.close(tokens, range, cut, CutReason::Commit, asr_ms, out);
                    self.consumed += k + 1;
                    // The rest of the text is the next clause, which starts at the cut.
                    if tokens.tokens[self.consumed..count]
                        .iter()
                        .any(|t| is_word_token(t))
                    {
                        self.open(cut, out);
                    }
                }
                Decision::None | Decision::WaitStable { .. } => break,
            }
        }
        if close {
            if self.clause.is_some() {
                let last = (self.consumed..count).next_back().map_or(audio_end, |i| {
                    StreamTime::from_seconds(at(i) + LAST_WORD_S).min(audio_end)
                });
                let range = self.consumed..count;
                self.close(tokens, range, last, reason, asr_ms, out);
            }
            self.consumed = count;
        } else if self.partials {
            self.partial(tokens, count, audio_end, out);
        }
    }

    fn open(&mut self, start: StreamTime, out: &mut Vec<StreamOutput>) {
        let id = UtteranceId(self.next_id);
        self.next_id += 1;
        self.clause = Some(Clause {
            id,
            start,
            last_partial: String::new(),
        });
        out.push(StreamOutput::Started(id, start));
    }

    fn partial(
        &mut self,
        tokens: &crate::engines::TokenTranscript,
        count: usize,
        end: StreamTime,
        out: &mut Vec<StreamOutput>,
    ) {
        let Some(clause) = self.clause.as_mut() else {
            return;
        };
        let text = clean(
            &join_tokens(&tokens.tokens[self.consumed..count]),
            &self.config.filter.fillers,
        );
        if text.is_empty() || text == clause.last_partial {
            return;
        }
        let Some(class) = classify_with_lang(&text, None) else {
            return;
        };
        clause.last_partial.clone_from(&text);
        out.push(StreamOutput::Message(AsrMessage::Partial {
            id: clause.id,
            text,
            class,
            end,
        }));
    }

    fn close(
        &mut self,
        tokens: &crate::engines::TokenTranscript,
        range: std::ops::Range<usize>,
        end: StreamTime,
        cut: CutReason,
        asr_ms: u32,
        out: &mut Vec<StreamOutput>,
    ) {
        let Some(clause) = self.clause.take() else {
            return;
        };
        let end = end.max(StreamTime(clause.start.0 + 1));
        let samples = self.slice(clause.start, end);
        let segment = Segment {
            id: clause.id,
            start: clause.start,
            end,
            samples: samples.into(),
            cut_reason: cut,
        };
        let log_probs = tokens.log_probs.get(range.clone()).unwrap_or_default();
        let transcript = Transcript {
            id: clause.id,
            text: join_tokens(&tokens.tokens[range]),
            lang_tag: tokens.lang_tag.clone(),
            class: TextClass::Chinese,
            event: None,
            timing: StageTiming::default(),
            absorbed: Vec::new(),
            cut,
            logprob: mean_log_prob(log_probs),
        };
        let now = self.end_of_audio();
        out.push(StreamOutput::Closed(clause.id, end, cut));
        out.push(StreamOutput::Message(finalize(
            transcript,
            &segment,
            asr_ms,
            now,
            &self.config,
        )));
        // Later clauses start after this one.
        self.drop_audio_before(end);
    }

    fn fail(&mut self, message: &str, out: &mut Vec<StreamOutput>) {
        if let Some(clause) = self.clause.take() {
            let end = self.end_of_audio().max(StreamTime(clause.start.0 + 1));
            out.push(StreamOutput::Closed(
                clause.id,
                end,
                CutReason::Discontinuity,
            ));
            out.push(StreamOutput::Message(AsrMessage::Failed(
                clause.id,
                message.to_owned(),
            )));
        }
    }

    fn slice(&self, start: StreamTime, end: StreamTime) -> Vec<f32> {
        let from = start.0.saturating_sub(self.audio_start.0) as usize;
        let to = (end.0.saturating_sub(self.audio_start.0) as usize).min(self.audio.len());
        self.audio.range(from.min(to)..to).copied().collect()
    }

    fn drop_audio_before(&mut self, at: StreamTime) {
        let count = (at.0.saturating_sub(self.audio_start.0) as usize).min(self.audio.len());
        self.audio.drain(..count);
        self.audio_start = StreamTime(self.audio_start.0 + count as u64);
    }

    /// Bound the buffer: an open clause keeps up to `MAX_CLAUSE_S`, otherwise `IDLE_KEEP_S`.
    fn trim(&mut self) {
        let end = self.end_of_audio();
        let keep = if self.clause.is_some() {
            MAX_CLAUSE_S
        } else {
            IDLE_KEEP_S
        };
        let limit = StreamTime(end.0.saturating_sub(StreamTime::from_seconds(keep).0));
        if limit > self.audio_start {
            self.drop_audio_before(limit);
        }
    }
}

fn elapsed_ms(started: Instant) -> u32 {
    started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{engines::TokenTranscript, error::Result};

    /// Emits each scripted token 0.5 s after its start time, like a chunked encoder. Its pause
    /// rule fires 1 s after the last token.
    struct ScriptedStream {
        script: Vec<(&'static str, f64)>,
        heard_s: f64,
        part_start_s: f64,
        part_first: usize,
    }

    impl ScriptedStream {
        fn emitted(&self) -> usize {
            self.script
                .iter()
                .take_while(|(_, at)| *at + 0.5 <= self.heard_s)
                .count()
        }
    }

    impl StreamingAsr for ScriptedStream {
        fn accept(&mut self, samples: &[f32]) -> Result<()> {
            self.heard_s += samples.len() as f64 / 16_000.0;
            Ok(())
        }
        fn result(&mut self) -> Result<StreamResult> {
            let part = &self.script[self.part_first..self.emitted().max(self.part_first)];
            Ok(StreamResult {
                start_s: self.part_start_s,
                tokens: TokenTranscript {
                    text: part.iter().map(|(t, _)| *t).collect(),
                    tokens: part.iter().map(|(t, _)| (*t).to_owned()).collect(),
                    timestamps: part
                        .iter()
                        .map(|(_, at)| (at - self.part_start_s) as f32)
                        .collect(),
                    log_probs: vec![-0.1; part.len()],
                    ..TokenTranscript::default()
                },
            })
        }
        fn is_endpoint(&mut self) -> bool {
            let emitted = self.emitted();
            emitted > self.part_first && self.heard_s - self.script[emitted - 1].1 > 1.0
        }
        fn reset(&mut self) {
            self.part_first = self.emitted();
            self.part_start_s = self.heard_s;
        }
        fn finish(&mut self) -> Result<StreamResult> {
            self.heard_s = f64::INFINITY;
            self.result()
        }
    }

    #[test]
    fn clauses_end_at_marks_and_pauses_without_losing_text_and_carry_their_own_audio() {
        let mut config = Config::default();
        config.latency.comma_min_tokens = 3;
        // Two sentences, a 2 s pause, and a last sentence that the end of the audio closes.
        let mut script = Vec::new();
        let mut at = 0.3;
        for token in [
            "今", "天", "天", "气", "很", "好", "。", "我", "们", "去", "公", "园", "，", "然",
            "后", "吃", "饭", "。",
        ] {
            script.push((token, at));
            at += 0.2;
        }
        at += 2.0;
        for token in ["好", "的", "没", "问", "题"] {
            script.push((token, at));
            at += 0.2;
        }
        let mut engine = ScriptedStream {
            script,
            heard_s: 0.0,
            part_start_s: 0.0,
            part_first: 0,
        };
        let mut recognizer = StreamRecognizer::new(&config, true);
        let mut out = Vec::new();
        let base = StreamTime::from_seconds(10.0);
        let total = (at * 16_000.0) as usize;
        for (index, chunk) in (0..total).collect::<Vec<_>>().chunks(1_600).enumerate() {
            let samples: Vec<f32> = chunk.iter().map(|i| *i as f32 / total as f32).collect();
            let input = StreamInput::Audio {
                at: StreamTime(base.0 + index as u64 * 1_600),
                samples,
            };
            recognizer.handle(&mut engine, input, &mut out);
        }
        recognizer.handle(
            &mut engine,
            StreamInput::Break {
                at: StreamTime(base.0 + total as u64),
                reason: CutReason::End,
            },
            &mut out,
        );

        let mut started = Vec::new();
        let mut finals = Vec::new();
        for item in &out {
            match item {
                StreamOutput::Started(id, at) => started.push((*id, *at)),
                StreamOutput::Closed(id, _, _) => assert!(started.iter().any(|(s, _)| s == id)),
                StreamOutput::Message(AsrMessage::Transcript(t, audio)) => {
                    assert!(
                        started.iter().any(|(s, _)| *s == t.id),
                        "final before start"
                    );
                    let length = t.timing.end.0 - t.timing.start.0;
                    assert_eq!(audio.len() as u64, length, "clause audio matches its times");
                    // The buffer holds sample index / total, so the first sample names the start.
                    let first = (audio[0] * total as f32).round() as u64;
                    assert!((base.0 + first).abs_diff(t.timing.start.0) <= 1);
                    finals.push((t.text.clone(), t.cut, t.timing.start, t.timing.end));
                }
                StreamOutput::Message(AsrMessage::Partial { .. }) => {}
                StreamOutput::Message(other) => panic!("unexpected {other:?}"),
            }
        }
        let texts: Vec<_> = finals.iter().map(|(text, ..)| text.as_str()).collect();
        assert_eq!(
            texts,
            ["今天天气很好。", "我们去公园，", "然后吃饭。", "好的没问题"]
        );
        let cuts: Vec<_> = finals.iter().map(|(_, cut, ..)| *cut).collect();
        assert_eq!(
            cuts,
            [
                CutReason::Commit,
                CutReason::Commit,
                CutReason::Pause,
                CutReason::End
            ]
        );
        // A commit's next clause starts exactly at the cut; clauses never overlap.
        assert_eq!(finals[1].2, finals[0].3);
        assert_eq!(finals[2].2, finals[1].3);
        assert!(finals[3].2 >= finals[2].3);
        let ids: Vec<_> = started.iter().map(|(id, _)| id.0).collect();
        assert_eq!(ids, [0, 1, 2, 3]);
    }
}
