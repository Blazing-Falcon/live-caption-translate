use crate::{
    config::Config,
    error::{Error, Result},
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

impl TokenTranscript {
    pub fn mean_log_prob(&self) -> Option<f32> {
        mean_log_prob(&self.log_probs)
    }
}

pub fn mean_log_prob(log_probs: &[f32]) -> Option<f32> {
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
    /// Token times are seconds from `start_s`. Emitted tokens never change.
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
    /// The language the engine detected, such as "zh" or "ja".
    pub lang: Option<String>,
}

/// A recognizer that decodes one whole window of audio (16 kHz mono) at a time.
pub trait OfflineAsr: Send {
    /// `language`: decode as this language (such as "zh") instead of detecting it.
    fn decode(&mut self, samples: &[f32], language: Option<&str>) -> Result<OfflineText>;
}

#[derive(Clone, Debug)]
pub struct TranslatorCaps {
    /// Accepts earlier lines in `TranslateRequest::context`.
    pub context: bool,
    pub max_input_chars: usize,
}

/// Built-in translators must check this on each blocking/streaming step.
/// Cooperative cancellation makes shutdown safe without abandoning worker threads.
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
    /// Earlier lines as (source, English shown), oldest first. Engines
    /// without `caps().context` get none.
    pub context: &'a [(String, String)],
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

type AsrFactory = fn(&Config) -> Result<Box<dyn StreamingAsr>>;
type TranslatorFactory = fn(&Config) -> Result<Box<dyn Translator>>;

#[derive(Default)]
pub struct EngineRegistry {
    asr: Option<AsrFactory>,
    translator: Option<TranslatorFactory>,
}

impl EngineRegistry {
    pub fn register_asr(&mut self, factory: AsrFactory) {
        self.asr = Some(factory);
    }
    pub fn register_translator(&mut self, factory: TranslatorFactory) {
        self.translator = Some(factory);
    }
    pub fn build_asr(&self, config: &Config) -> Result<Box<dyn StreamingAsr>> {
        self.asr.ok_or_else(|| Error::UnknownEngine {
            kind: "ASR",
            name: "xasr_stream".into(),
        })?(config)
    }
    pub fn build_translator(&self, config: &Config) -> Result<Box<dyn Translator>> {
        self.translator.ok_or_else(|| Error::UnknownEngine {
            kind: "translator",
            name: "hymt2".into(),
        })?(config)
    }
}

pub fn max_tokens(text: &str, cap: u32) -> u32 {
    (text.chars().count().saturating_mul(4).saturating_add(16)).min(cap as usize) as u32
}
