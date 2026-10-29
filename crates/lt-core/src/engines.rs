use crate::{
    error::{Error, Result},
    screen::{ImageView, Rect},
    types::UtteranceId,
};
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
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub rect: Rect,
    pub text: String,
    /// Mean probability of the recognized characters, 0 to 1.
    pub score: f32,
}

/// Reads the text in a still image.
pub trait TextRecognizer: Send {
    /// Lines of text in `image`, rects in its pixels, in no particular order.
    fn recognize(&mut self, image: &ImageView<'_>) -> Result<Vec<TextLine>>;
}

#[derive(Clone, Debug)]
pub struct TranslatorCaps {
    pub max_input_chars: usize,
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
