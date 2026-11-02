use crate::{
    error::{Error, Result},
    screen::{ImageView, Rect},
    types::UtteranceId,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

/// Recognized tokens with per-token start times.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenTranscript {
    /// The recognizer's full text.
    pub text: String,
    /// In order; English word starts carry a leading space (" school").
    pub tokens: Vec<String>,
    /// Token start times in seconds.
    pub timestamps: Vec<f32>,
    /// Per-token log-probabilities, parallel to `tokens`; empty when the engine gives none.
    pub log_probs: Vec<f32>,
}

pub(crate) fn mean_log_prob(log_probs: &[f32]) -> Option<f32> {
    let finite: Vec<f32> = log_probs
        .iter()
        .copied()
        .filter(|p| p.is_finite())
        .collect();
    (!finite.is_empty()).then(|| finite.iter().sum::<f32>() / finite.len() as f32)
}

/// What a streaming engine recognized since its last reset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamResult {
    /// When this part began, in seconds since the stream started.
    pub start_s: f64,
    /// Token times are seconds from `start_s`. Emitted tokens never change; a later result only
    /// adds tokens.
    pub tokens: TokenTranscript,
}

/// A recognizer that keeps its state across pieces of audio (16 kHz mono).
pub trait StreamingAsr: Send {
    /// Add audio and decode what is ready.
    fn accept(&mut self, samples: &[f32]) -> Result<()>;
    fn result(&mut self) -> Result<StreamResult>;
    /// The engine's own pause rules found the end of an utterance.
    fn is_endpoint(&mut self) -> bool;
    /// Start a new part at the current position, keeping the stream clock.
    fn reset(&mut self);
    /// Decode the remaining audio and return the last result. The next `accept` starts a new
    /// stream at time 0.
    fn finish(&mut self) -> Result<StreamResult>;
}

/// What an offline recognizer heard in one window of audio.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OfflineText {
    pub text: String,
}

/// A recognizer that decodes one whole window of audio (16 kHz mono) at a time.
pub trait OfflineAsr: Send {
    fn decode(&mut self, samples: &[f32]) -> Result<OfflineText>;

    /// Seconds of audio before a clause's window that the engine also reads. Earlier speech gives
    /// a short clause the context its words depend on; `trim` leaves out the text it adds.
    fn context_s(&self) -> f64 {
        0.0
    }
}

/// One line of text found in an image.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TextLine {
    pub rect: Rect,
    pub text: String,
    /// Mean probability of the recognized characters, 0 to 1.
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quad: Option<[crate::screen::geom::PointF; 4]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub char_scores: Vec<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyph_pitch: Option<f64>,
}

/// Reads the text in a still image.
pub trait TextRecognizer: Send {
    /// Lines of text in `image`, rects in its pixels, in no particular order.
    fn recognize(&mut self, image: &ImageView<'_>) -> Result<Vec<TextLine>>;
    fn recognize_direction(
        &mut self,
        image: &ImageView<'_>,
        _direction: crate::screen::Direction,
    ) -> Result<Vec<TextLine>> {
        self.recognize(image)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PreviewMode {
    #[default]
    Live,
    Clauses,
}

#[derive(Clone, Debug)]
pub struct TranslatorCaps {
    pub max_input_chars: usize,
    pub previews: PreviewMode,
    pub max_batch_items: usize,
    pub max_batch_chars: usize,
}

impl Default for TranslatorCaps {
    fn default() -> Self {
        Self {
            max_input_chars: 300,
            previews: PreviewMode::Live,
            max_batch_items: 1,
            max_batch_chars: 300,
        }
    }
}

/// Built-in translators must check this on each blocking or streaming step, so shutdown never
/// has to abandon a worker thread.
#[derive(Clone, Debug)]
pub struct TranslationControl {
    pub deadline: Instant,
    pub cancelled: Arc<AtomicBool>,
    pub abort: Arc<AtomicBool>,
}

impl TranslationControl {
    pub fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(Error::Stopped);
        }
        if self.abort.load(Ordering::Relaxed) {
            return Err(Error::Translation {
                reason: crate::events::FailReason::Runaway,
                message: "Translation started repeating".into(),
            });
        }
        if Instant::now() >= self.deadline {
            return Err(Error::Translation {
                reason: crate::events::FailReason::Timeout,
                message: "Translation took too long".into(),
            });
        }
        Ok(())
    }
}

pub struct TranslateRequest<'a> {
    pub id: UtteranceId,
    /// Chinese or mixed source text; the output is English.
    pub text: &'a str,
    /// Overrides the engine's default output budget.
    pub max_tokens: Option<u32>,
    pub control: TranslationControl,
}

#[derive(Clone, Debug, Default)]
pub struct TranslationOut {
    pub text: String,
    pub prompt_tokens: u32,
    pub cached_tokens: u32,
    pub generated_tokens: u32,
    /// The server stopped at the output budget instead of finishing the sentence.
    pub truncated: bool,
}

pub trait Translator: Send {
    fn caps(&self) -> TranslatorCaps;
    fn warm_up(&mut self, control: &TranslationControl) -> Result<()>;
    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut>;
    fn translate_batch(
        &mut self,
        texts: &[&str],
        control: &TranslationControl,
    ) -> Result<Vec<TranslationOut>> {
        texts
            .iter()
            .map(|text| {
                control.check()?;
                self.translate(
                    &TranslateRequest {
                        id: UtteranceId(0),
                        text,
                        max_tokens: None,
                        control: control.clone(),
                    },
                    &mut |_| {},
                )
            })
            .collect()
    }
}

/// One request's time limit, including the stream.
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Above `2 * 120 + 8`, the budget of the longest line the queue joins, so only a runaway
/// answer reaches it.
const MAX_TOKENS_CAP: u32 = 256;

/// An output budget from the source length: about two tokens per source character.
pub fn max_tokens(text: &str) -> u32 {
    let wanted = text.chars().count().saturating_mul(2).saturating_add(8);
    u32::try_from(wanted)
        .unwrap_or(u32::MAX)
        .clamp(24, MAX_TOKENS_CAP)
}

/// An answer this many times longer than its source in characters is not a translation of it.
const MAX_LENGTH_RATIO: usize = 12;
/// A one- or two-character word can need a long phrase: 高考 is "College entrance examination".
const MIN_LENGTH_LIMIT: usize = 40;
/// Openings of a reply that answers the instruction instead of translating the line. They are
/// lowercase because the check lowercases the answer first.
const REPLY_PHRASES: [&str; 6] = [
    "please translate",
    "translate the following",
    "here is the translation",
    "here's the translation",
    "the translation is",
    "translated text",
];
/// A reply introduces what follows with a colon soon after its opening. A translation that only
/// contains the words ("Please translate this.", "The translation is wrong.") has none.
const REPLY_COLON_WITHIN: usize = 40;

pub fn answer_problem(output: &TranslationOut, source_text: &str) -> Option<&'static str> {
    if output.text.is_empty() {
        return Some("empty");
    }
    let source = source_text.chars().count().max(1);
    if output.text.chars().count() > (MAX_LENGTH_RATIO * source).max(MIN_LENGTH_LIMIT) {
        return Some("longer than a translation");
    }
    // The model sometimes answers the instruction instead of translating: "Please translate the
    // following content into English: <line>". That is a reply, not a translation, and the line
    // it repeats is already in the answer.
    let lowered = output.text.to_lowercase();
    let replies = REPLY_PHRASES.iter().any(|phrase| {
        lowered.find(phrase).is_some_and(|at| {
            lowered[at + phrase.len()..]
                .chars()
                .take(REPLY_COLON_WITHIN)
                .any(|character| matches!(character, ':' | '：'))
        })
    });
    if replies {
        return Some("answered instead of translating");
    }
    None
}
