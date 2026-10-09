use crate::{
    config::Config,
    error::{Error, Result},
    types::{Segment, Transcript, UtteranceId},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

pub trait Vad: Send {
    fn speech_prob(&mut self, frame: &[f32; 512]) -> f32;
    fn reset(&mut self);
}

pub trait SegmentAsr: Send {
    fn transcribe(&mut self, segment: &Segment) -> Result<Transcript>;

    /// The windowed decode used by the recognizer. Engines without it keep the segment behavior:
    /// the pipeline falls back to Off mode.
    fn window(&mut self) -> Option<&mut dyn WindowAsr> {
        None
    }
}

/// One recognizer run over a window of audio, with per-token start times.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenTranscript {
    /// The recognizer's full text for the window.
    pub text: String,
    /// In order; English word starts carry a leading space (" school").
    pub tokens: Vec<String>,
    /// Token start times in seconds from the window start.
    pub timestamps: Vec<f32>,
    pub lang_tag: Option<String>,
    pub event: Option<String>,
}

pub trait WindowAsr: Send {
    fn decode_window(&mut self, samples: &[f32]) -> Result<TokenTranscript>;
}

pub trait StreamingAsr: Send {
    fn accept(&mut self, samples: &[f32]);
    fn partial(&mut self) -> Option<String>;
    fn reset(&mut self);
}

#[derive(Clone, Debug)]
pub struct TranslatorCaps {
    pub streaming: bool,
    pub glossary: bool,
    pub context: bool,
    /// Accepts an answer prefix (`TranslateRequest::prefill`), used for drafts.
    pub prefill: bool,
    pub max_input_chars: usize,
    pub pairs: Vec<(String, String)>,
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
    pub text: &'a str,
    pub src: &'a str,
    pub tgt: &'a str,
    pub terms: &'a [(String, String)],
    pub context: &'a [(String, String)],
    /// English the answer must start with; "" for none. Only draft engines honor it.
    pub prefill: &'a str,
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

type AsrFactory = fn(&Config) -> Result<Box<dyn SegmentAsr>>;
type VadFactory = fn(&Config) -> Result<Box<dyn Vad>>;
type TranslatorFactory = fn(&Config) -> Result<Box<dyn Translator>>;

#[derive(Default)]
pub struct EngineRegistry {
    asr: BTreeMap<String, AsrFactory>,
    vad: BTreeMap<String, VadFactory>,
    translators: BTreeMap<String, TranslatorFactory>,
    draft_translators: BTreeMap<String, TranslatorFactory>,
}

impl EngineRegistry {
    pub fn register_asr(&mut self, name: &str, factory: AsrFactory) {
        self.asr.insert(name.into(), factory);
    }
    pub fn register_vad(&mut self, name: &str, factory: VadFactory) {
        self.vad.insert(name.into(), factory);
    }
    pub fn register_translator(&mut self, name: &str, factory: TranslatorFactory) {
        self.translators.insert(name.into(), factory);
    }
    pub fn register_draft_translator(&mut self, name: &str, factory: TranslatorFactory) {
        self.draft_translators.insert(name.into(), factory);
    }
    pub fn build_asr(&self, config: &Config) -> Result<Box<dyn SegmentAsr>> {
        self.asr
            .get(&config.asr.engine)
            .ok_or_else(|| Error::UnknownEngine {
                kind: "ASR",
                name: config.asr.engine.clone(),
            })?(config)
    }
    pub fn build_vad(&self, config: &Config) -> Result<Box<dyn Vad>> {
        self.vad
            .get(&config.vad.engine)
            .ok_or_else(|| Error::UnknownEngine {
                kind: "VAD",
                name: config.vad.engine.clone(),
            })?(config)
    }
    pub fn build_translator(&self, config: &Config) -> Result<Box<dyn Translator>> {
        self.translators
            .get(&config.translate.engine)
            .ok_or_else(|| Error::UnknownEngine {
                kind: "translator",
                name: config.translate.engine.clone(),
            })?(config)
    }
    pub fn build_draft_translator(&self, config: &Config) -> Result<Box<dyn Translator>> {
        self.draft_translators
            .get(&config.latency.draft_engine)
            .ok_or_else(|| Error::UnknownEngine {
                kind: "draft translator",
                name: config.latency.draft_engine.clone(),
            })?(config)
    }
}

pub fn max_tokens(text: &str, cap: u32) -> u32 {
    (text.chars().count().saturating_mul(4).saturating_add(16)).min(cap as usize) as u32
}
