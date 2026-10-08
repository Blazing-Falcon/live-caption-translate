//! Deterministic engines for headless replay and integration tests.
use crate::{
    engines::{
        SegmentAsr, TranslateRequest, TranslationControl, TranslationOut, Translator,
        TranslatorCaps, Vad,
    },
    error::Result,
    types::{Segment, StageTiming, TextClass, Transcript},
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

pub struct FakeVad {
    probabilities: Vec<f32>,
    index: usize,
    threshold_energy: f32,
}

impl FakeVad {
    pub fn from_probabilities(probabilities: Vec<f32>) -> Self {
        Self {
            probabilities,
            index: 0,
            threshold_energy: 0.001,
        }
    }
    pub fn from_energy(threshold_energy: f32) -> Self {
        Self {
            probabilities: Vec::new(),
            index: 0,
            threshold_energy,
        }
    }
}

impl Vad for FakeVad {
    fn speech_prob(&mut self, frame: &[f32; 512]) -> f32 {
        let prob = self
            .probabilities
            .get(self.index)
            .copied()
            .unwrap_or_else(|| {
                if frame.iter().map(|sample| sample * sample).sum::<f32>() / 512.0
                    > self.threshold_energy
                {
                    0.9
                } else {
                    0.0
                }
            });
        self.index += 1;
        prob
    }
    fn reset(&mut self) {
        self.index = 0;
    }
}

pub struct FakeAsr {
    texts: Vec<(String, Option<String>, Option<String>)>,
    index: usize,
}

impl FakeAsr {
    pub fn new(texts: Vec<String>) -> Self {
        Self {
            texts: texts
                .into_iter()
                .map(|text| (text, Some("zh".into()), Some("Speech".into())))
                .collect(),
            index: 0,
        }
    }
    pub fn with_metadata(texts: Vec<(String, Option<String>, Option<String>)>) -> Self {
        Self { texts, index: 0 }
    }
}

impl SegmentAsr for FakeAsr {
    fn transcribe(&mut self, segment: &Segment) -> Result<Transcript> {
        let (text, lang, event) = self.texts.get(self.index).cloned().unwrap_or_default();
        self.index += 1;
        Ok(Transcript {
            id: segment.id,
            text,
            lang_tag: lang,
            class: TextClass::Chinese,
            event,
            timing: StageTiming {
                start: segment.start,
                end: segment.end,
                asr_done_ms: 0,
                asr_ms: 0,
            },
            absorbed: Vec::new(),
            cut: segment.cut_reason,
        })
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
