//! Deterministic engines for headless replay and integration tests.
use crate::{
    engines::{
        StreamResult, StreamingAsr, TokenTranscript, TranslateRequest, TranslationControl,
        TranslationOut, Translator, TranslatorCaps,
    },
    error::Result,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

/// A streaming recognizer that hears loud audio as speech. Each burst of speech adds the next
/// text of its list, one token per character, all timed at the burst's first loud piece. Its
/// pause rule fires after `pause_s` of quiet audio following a burst.
pub struct FakeStream {
    texts: Vec<String>,
    next: usize,
    threshold_energy: f32,
    pause_samples: u64,
    /// Samples heard since the stream started.
    heard: u64,
    /// Start of the current part, in samples since the stream started.
    part_start: u64,
    /// The current part's tokens with their start (samples since the part started).
    tokens: Vec<(String, u64)>,
    quiet: u64,
}

impl FakeStream {
    pub fn new(texts: Vec<String>, pause_s: f64) -> Self {
        Self {
            texts,
            next: 0,
            threshold_energy: 0.001,
            pause_samples: (pause_s * 16_000.0).round() as u64,
            heard: 0,
            part_start: 0,
            tokens: Vec::new(),
            quiet: 0,
        }
    }
}

impl StreamingAsr for FakeStream {
    fn accept(&mut self, samples: &[f32]) -> Result<()> {
        let energy =
            samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len().max(1) as f32;
        if energy > self.threshold_energy {
            if self.tokens.is_empty() || self.quiet > 0 {
                let text = self.texts.get(self.next).cloned().unwrap_or_default();
                self.next += 1;
                let at = self.heard - self.part_start;
                self.tokens
                    .extend(text.chars().map(|c| (c.to_string(), at)));
            }
            self.quiet = 0;
        } else {
            self.quiet += samples.len() as u64;
        }
        self.heard += samples.len() as u64;
        Ok(())
    }

    fn result(&mut self) -> Result<StreamResult> {
        Ok(StreamResult {
            start_s: self.part_start as f64 / 16_000.0,
            tokens: TokenTranscript {
                text: self
                    .tokens
                    .iter()
                    .map(|(token, _)| token.as_str())
                    .collect(),
                tokens: self.tokens.iter().map(|(token, _)| token.clone()).collect(),
                timestamps: self
                    .tokens
                    .iter()
                    .map(|(_, at)| *at as f32 / 16_000.0)
                    .collect(),
                log_probs: vec![-0.1; self.tokens.len()],
            },
        })
    }

    fn is_endpoint(&mut self) -> bool {
        !self.tokens.is_empty() && self.quiet >= self.pause_samples
    }

    fn reset(&mut self) {
        self.part_start = self.heard;
        self.tokens.clear();
    }

    fn finish(&mut self) -> Result<StreamResult> {
        let result = self.result();
        self.heard = 0;
        self.part_start = 0;
        self.tokens.clear();
        self.quiet = 0;
        result
    }
}

#[derive(Default)]
pub struct FakeTranslatorMetrics {
    pub warmups: AtomicUsize,
    pub requests: AtomicUsize,
    pub active: AtomicUsize,
    pub max_active: AtomicUsize,
}

pub struct FakeTranslator {
    pub responses: BTreeMap<String, String>,
    pub delay_per_character: Duration,
    pub delta_delay: Duration,
    pub timeout_text: Option<String>,
    pub metrics: Arc<FakeTranslatorMetrics>,
}

impl Default for FakeTranslator {
    fn default() -> Self {
        Self {
            responses: BTreeMap::new(),
            delay_per_character: Duration::ZERO,
            delta_delay: Duration::ZERO,
            timeout_text: None,
            metrics: Arc::new(FakeTranslatorMetrics::default()),
        }
    }
}

fn cooperative_sleep(duration: Duration, control: &TranslationControl) -> Result<()> {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        control.check()?;
        thread::sleep(
            deadline
                .saturating_duration_since(std::time::Instant::now())
                .min(Duration::from_millis(5)),
        );
    }
    control.check()
}

struct ActiveGuard(Arc<FakeTranslatorMetrics>);
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Translator for FakeTranslator {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            streaming: true,
            glossary: false,
            context: false,
            prefill: false,
            max_input_chars: 300,
            pairs: vec![("zh".into(), "en".into())],
        }
    }
    fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
        control.check()?;
        self.metrics.warmups.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        let active = self.metrics.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _guard = ActiveGuard(self.metrics.clone());
        self.metrics.max_active.fetch_max(active, Ordering::SeqCst);
        self.metrics.requests.fetch_add(1, Ordering::SeqCst);
        if self.timeout_text.as_deref() == Some(request.text) {
            loop {
                request.control.check()?;
                thread::sleep(Duration::from_millis(5));
            }
        }
        cooperative_sleep(
            self.delay_per_character
                .saturating_mul(request.text.chars().count() as u32),
            &request.control,
        )?;
        let text = self
            .responses
            .get(request.text)
            .cloned()
            .unwrap_or_else(|| "Hello.".into());
        let mut accumulated = String::new();
        for (index, word) in text.split_whitespace().enumerate() {
            cooperative_sleep(self.delta_delay, &request.control)?;
            if index > 0 {
                accumulated.push(' ');
            }
            accumulated.push_str(word);
            on_delta(&accumulated);
        }
        Ok(TranslationOut {
            text,
            prompt_tokens: 17,
            cached_tokens: 31,
            generated_tokens: accumulated.split_whitespace().count() as u32,
        })
    }
}
