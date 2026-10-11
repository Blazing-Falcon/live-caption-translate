//! Clauses from a streaming recognizer that hears all audio continuously.
//!
//! The engine's text only grows. A clause opens at its first word and ends at a commit
//! (punctuation or the wait cap), at the engine's pause rule, or at a break in the audio. Each
//! clause gets an id and start and close notices for the scheduler and joiner.
//!
//! `StreamRecognizer` is a plain state machine; the `lt-asr` thread only moves its inputs and
//! outputs.

use crate::{
    commit::{cut_at, decide, is_word_token, join_tokens, CommitSettings, Decision, MIN_CLAUSE_S},
    config::Config,
    engines::{mean_log_prob, StreamResult, StreamingAsr},
    events::DropReason,
    text::{chinese_chars, classify, clean, drop_reason},
    types::{CutReason, StageTiming, StreamTime, TextClass, Transcript, UtteranceId},
};
use std::time::Instant;

/// A clause starts this long before its first token: the token time is where the word was
/// recognized, and speech onset comes a little before it.
const PRE_ROLL_S: f64 = 0.3;
/// A clause that ends at a pause or break ends this long after its last token start, so its
/// end time covers the last word.
const LAST_WORD_S: f64 = 0.6;
/// A clause this old may also end at a comma. Casual speech often runs on with commas for half a
/// minute without a sentence end; before this age a comma usually splits a short phrase.
const COMMA_AFTER_S: f64 = 2.0;
/// A clause this old without punctuation is cut at the widest gap between words, so translation
/// input stays bounded.
const CAP_AFTER_S: f64 = 30.0;

/// The commit rule for a clause whose newest word is `age_s` seconds after its start: sentence
/// marks always, commas after `COMMA_AFTER_S`, the wait cap after `CAP_AFTER_S`.
fn rules(age_s: f64) -> CommitSettings {
    let rules = CommitSettings::STREAMING;
    if age_s < COMMA_AFTER_S {
        CommitSettings {
            comma_min_tokens: usize::MAX,
            split_long: false,
            ..rules
        }
    } else if age_s < CAP_AFTER_S {
        CommitSettings {
            split_long: false,
            ..rules
        }
    } else {
        rules
    }
}

/// What the frontend sends in streaming mode.
#[derive(Clone, Debug)]
pub enum StreamInput {
    /// Contiguous audio starting at `at`.
    Audio { at: StreamTime, samples: Vec<f32> },
    /// The audio stops or jumps (pause, device change, end of file).
    Break { at: StreamTime, reason: CutReason },
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
    next_id: u64,
    /// Stream time of engine time 0; `None` until audio arrives after a break.
    origin: Option<StreamTime>,
    /// The end of the audio heard so far.
    heard: StreamTime,
    /// The next clause starts no earlier: clauses never overlap.
    floor: StreamTime,
    clause: Option<Clause>,
    /// Tokens of the engine's current part that belong to finished clauses.
    consumed: usize,
}

impl StreamRecognizer {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
            next_id: 0,
            origin: None,
            heard: StreamTime::ZERO,
            floor: StreamTime::ZERO,
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
                    self.floor = at;
                }
                self.heard = StreamTime(at.0 + samples.len() as u64);
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
            }
            StreamInput::Break { reason, .. } => {
                let started = Instant::now();
                match engine.finish() {
                    Ok(result) => self.process(&result, elapsed_ms(started), true, reason, out),
                    Err(error) => self.fail(&error.to_string(), out),
                }
                self.origin = None;
                self.consumed = 0;
            }
        }
    }

    fn end_of_audio(&self) -> StreamTime {
        self.heard
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
                    let start = StreamTime::from_seconds((at(first_word) - PRE_ROLL_S).max(0.0))
                        .max(self.floor);
                    self.open(start, out);
                    start
                }
            }
            .seconds();
            let words: Vec<String> = tokens.tokens[rest.clone()].to_vec();
            let times: Vec<f64> = rest.clone().map(|i| at(i) - clause_start).collect();
            let age = times.last().copied().unwrap_or_default();
            match decide(&words, &times, &rules(age)) {
                Decision::Commit { k } | Decision::Cap { k } => {
                    let cut = StreamTime::from_seconds(
                        cut_at(clause_start, times[k + 1], audio_end.seconds())
                            .max(clause_start + MIN_CLAUSE_S)
                            .min(audio_end.seconds()),
                    );
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
                Decision::None => break,
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
        } else {
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
        let Some(class) = classify(&text) else {
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
        let log_probs = tokens.log_probs.get(range.clone()).unwrap_or_default();
        let transcript = Transcript {
            id: clause.id,
            text: join_tokens(&tokens.tokens[range]),
            lang_tag: None,
            class: TextClass::Chinese,
            timing: StageTiming {
                start: clause.start,
                end,
                asr_done_ms: self.end_of_audio().millis(),
                asr_ms,
            },
            absorbed: Vec::new(),
            cut,
            logprob: mean_log_prob(log_probs),
        };
        out.push(StreamOutput::Closed(clause.id, end, cut));
        out.push(StreamOutput::Message(finalize(transcript, &self.config)));
        // Later clauses start after this one.
        self.floor = self.floor.max(end);
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
}

fn elapsed_ms(started: Instant) -> u32 {
    started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

/// Clean, classify and filter a closed clause.
fn finalize(mut transcript: Transcript, config: &Config) -> AsrMessage {
    transcript.text = clean(&transcript.text, &config.filter.fillers);
    let Some(class) = classify(&transcript.text) else {
        return AsrMessage::Dropped(transcript.id, DropReason::Empty);
    };
    transcript.class = class;
    transcript.lang_tag = if class == TextClass::Other {
        other_language(&transcript.text)
    } else {
        text_language(&transcript.text)
    };
    let duration = transcript
        .timing
        .end
        .saturating_sub(transcript.timing.start);
    match drop_reason(&transcript, duration.seconds(), &config.filter) {
        Some(reason) => AsrMessage::Dropped(transcript.id, reason),
        None => AsrMessage::Transcript(transcript),
    }
}

/// Japanese or Korean, from the kana and Hangul in the text.
fn other_language(text: &str) -> Option<String> {
    let kana = text
        .chars()
        .filter(|character| matches!(character, '\u{3040}'..='\u{30ff}'))
        .count();
    let hangul = text
        .chars()
        .filter(|character| matches!(character, '\u{ac00}'..='\u{d7af}'))
        .count();
    if hangul > kana {
        Some("ko".into())
    } else if kana > 0 {
        Some("ja".into())
    } else {
        None
    }
}

/// Language from the share of Latin letters and CJK characters; the engine gives no tag.
fn text_language(text: &str) -> Option<String> {
    let latin = text.chars().filter(char::is_ascii_alphabetic).count();
    let cjk = chinese_chars(text);
    match (latin, cjk) {
        (0, 0) => None,
        _ if latin > cjk => Some("en".into()),
        _ => Some("zh".into()),
    }
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

    type Final = (String, CutReason, StreamTime, StreamTime);

    /// Feeds `script` (token, start seconds) in 100 ms pieces and ends the audio right after the
    /// last token is emitted, before the pause rule can fire.
    fn run(script: Vec<(&'static str, f64)>) -> (Vec<Final>, Vec<u64>, usize) {
        let end_s = script.last().map_or(0.0, |(_, at)| at + 0.6);
        let mut engine = ScriptedStream {
            script,
            heard_s: 0.0,
            part_start_s: 0.0,
            part_first: 0,
        };
        let mut recognizer = StreamRecognizer::new(&Config::default());
        let mut out = Vec::new();
        let base = StreamTime::from_seconds(10.0);
        let total = (end_s * 16_000.0) as usize;
        for (index, chunk) in (0..total).collect::<Vec<_>>().chunks(1_600).enumerate() {
            let input = StreamInput::Audio {
                at: StreamTime(base.0 + index as u64 * 1_600),
                samples: vec![0.0; chunk.len()],
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
        let mut partials = 0;
        for item in &out {
            match item {
                StreamOutput::Started(id, at) => started.push((*id, *at)),
                StreamOutput::Closed(id, _, _) => assert!(started.iter().any(|(s, _)| s == id)),
                StreamOutput::Message(AsrMessage::Transcript(t)) => {
                    assert!(
                        started.iter().any(|(s, _)| *s == t.id),
                        "final before start"
                    );
                    finals.push((t.text.clone(), t.cut, t.timing.start, t.timing.end));
                }
                StreamOutput::Message(AsrMessage::Partial { .. }) => partials += 1,
                StreamOutput::Message(other) => panic!("unexpected {other:?}"),
            }
        }
        let ids = started.iter().map(|(id, _)| id.0).collect();
        (finals, ids, partials)
    }

    /// `tokens` one after another every `step_s` seconds, starting at `at`.
    fn spaced(tokens: &[&'static str], at: f64, step_s: f64) -> Vec<(&'static str, f64)> {
        tokens
            .iter()
            .enumerate()
            .map(|(index, token)| (*token, at + index as f64 * step_s))
            .collect()
    }

    /// Clauses end at a sentence mark and at a pause, not at an early comma; the live text shows
    /// while a clause is open, and clauses never overlap.
    #[test]
    fn clauses_end_at_sentence_marks_and_pauses_and_show_live_text() {
        let mut script = spaced(
            &[
                "今", "天", "天", "气", "很", "好", "。", "我", "们", "去", "公", "园", "，", "然",
                "后", "吃", "饭", "。",
            ],
            0.3,
            0.2,
        );
        script.extend(spaced(&["好", "的", "没", "问", "题"], 5.9, 0.2));
        let (finals, ids, partials) = run(script);
        let texts: Vec<_> = finals.iter().map(|(text, ..)| text.as_str()).collect();
        assert_eq!(
            texts,
            ["今天天气很好。", "我们去公园，然后吃饭。", "好的没问题"]
        );
        let cuts: Vec<_> = finals.iter().map(|(_, cut, ..)| *cut).collect();
        assert_eq!(cuts, [CutReason::Commit, CutReason::Pause, CutReason::End]);
        // A commit's next clause starts exactly at the cut.
        assert_eq!(finals[1].2, finals[0].3);
        assert!(finals[2].2 >= finals[1].3);
        assert_eq!(ids, [0, 1, 2]);
        assert!(partials > 0);
    }

    /// A comma ends a run-on clause only once the clause is `COMMA_AFTER_S` old.
    #[test]
    fn a_comma_ends_a_clause_only_after_the_comma_age() {
        let words = ["我", "们", "今", "天", "去", "公", "园", "玩", "了", "吧"];
        // Ten words `step` apart, a comma, and four more words. The scripted pause rule needs
        // gaps under 0.5 s.
        let clause = |step: f64| {
            let mut script = spaced(&words, 0.3, step);
            let comma = 0.3 + 10.0 * step;
            script.push(("，", comma));
            script.extend(spaced(&["然", "后", "回", "家"], comma + step, step));
            run(script)
                .0
                .into_iter()
                .map(|(text, cut, ..)| (text, cut))
                .collect::<Vec<_>>()
        };
        // The whole clause is younger than the age: the comma stays inside it.
        assert_eq!(
            clause(COMMA_AFTER_S / 20.0),
            [("我们今天去公园玩了吧，然后回家".to_owned(), CutReason::End)]
        );
        // The words after the comma come after the age: the comma ends the clause.
        assert_eq!(
            clause(COMMA_AFTER_S / 9.0),
            [
                ("我们今天去公园玩了吧，".to_owned(), CutReason::Commit),
                ("然后回家".to_owned(), CutReason::End)
            ]
        );
    }
}
