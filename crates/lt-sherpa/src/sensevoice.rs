//! SenseVoice through sherpa's offline recognizer; keeps its language and event tags.

use crate::offline::{
    cstring, path_cstring, require_threads, zeroed_config, OfflineEngine, RawResult,
};
use lt_core::{
    config::AsrConfig,
    engines::{SegmentAsr, TokenTranscript, WindowAsr},
    error::Result,
    text::{classify_with_lang, clean},
    types::{Segment, StageTiming, TextClass, Transcript},
};
use std::{path::Path, time::Instant};

const NAME: &str = "SenseVoice";

pub struct SenseVoiceAsr {
    engine: OfflineEngine,
}

impl SenseVoiceAsr {
    pub fn new(
        model: impl AsRef<Path>,
        tokens: impl AsRef<Path>,
        settings: &AsrConfig,
    ) -> Result<Self> {
        Self::open(model.as_ref(), tokens.as_ref(), settings, None)
    }

    pub fn with_runtime(
        model: impl AsRef<Path>,
        tokens: impl AsRef<Path>,
        settings: &AsrConfig,
        runtime: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open(
            model.as_ref(),
            tokens.as_ref(),
            settings,
            Some(runtime.as_ref()),
        )
    }

    fn open(
        model: &Path,
        tokens: &Path,
        settings: &AsrConfig,
        runtime: Option<&Path>,
    ) -> Result<Self> {
        require_threads(NAME, settings.threads)?;
        OfflineEngine::prepare(NAME, runtime, &[model, tokens])?;
        let model = path_cstring(model, "SenseVoice model path")?;
        let tokens = path_cstring(tokens, "SenseVoice tokens path")?;
        let language = cstring(&settings.language)?;
        let provider = cstring("cpu")?;
        let decoding = cstring("greedy_search")?;
        let mut config = zeroed_config();
        config.model_config.tokens = tokens.as_ptr();
        config.model_config.num_threads = settings.threads as i32;
        config.model_config.provider = provider.as_ptr();
        config.model_config.sense_voice.model = model.as_ptr();
        config.model_config.sense_voice.language = language.as_ptr();
        config.model_config.sense_voice.use_itn = i32::from(settings.use_itn);
        config.decoding_method = decoding.as_ptr();
        config.max_active_paths = 4;
        Ok(Self {
            engine: OfflineEngine::create(NAME, &config)?,
        })
    }

    /// Native JSON is retained for explicit local metadata diagnostics and
    /// parity tests. It is never logged automatically.
    pub fn last_result_json(&self) -> Option<&str> {
        self.engine.last_result_json()
    }
}

impl WindowAsr for SenseVoiceAsr {
    fn decode_window(&mut self, samples: &[f32]) -> Result<TokenTranscript> {
        Ok(self.engine.decode(samples)?.into_tokens())
    }
}

impl SegmentAsr for SenseVoiceAsr {
    fn window(&mut self) -> Option<&mut dyn WindowAsr> {
        Some(self)
    }

    fn transcribe(&mut self, segment: &Segment) -> Result<Transcript> {
        let started = Instant::now();
        let raw: RawResult = self.engine.decode(&segment.samples)?;
        let clean_text = clean(&raw.text, &[]);
        let class =
            classify_with_lang(&clean_text, raw.lang.as_deref()).unwrap_or(TextClass::Chinese);
        Ok(Transcript {
            id: segment.id,
            text: raw.text,
            lang_tag: raw.lang,
            class,
            event: raw.event,
            timing: StageTiming {
                start: segment.start,
                end: segment.end,
                asr_done_ms: 0,
                asr_ms: started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
            },
            absorbed: Vec::new(),
            cut: segment.cut_reason,
            logprob: None,
        })
    }
}
