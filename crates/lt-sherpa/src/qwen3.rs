//! Qwen3-ASR 0.6B: an audio encoder with a small language model that writes the text. It reads
//! each window whole and knows which words fit together, so it fixes more homophones than
//! SenseVoice, at several times the CPU.

use crate::{
    offline::{zeroed_config, OfflineRecognizer},
    runtime::{cstring, path_cstring, prepare},
};
use lt_core::{
    config::AsrConfig,
    engines::{OfflineAsr, OfflineText},
    error::Result,
};
use std::path::Path;

const NAME: &str = "Qwen3-ASR";
/// Prompt, audio and text tokens of one decode. Each second of audio takes about 13 tokens, so
/// a 30 s clause with its margins fits. The key-value cache is allocated for this many tokens
/// (28 layers, about 117 MB), so it is not larger.
const MAX_TOTAL_TOKENS: i32 = 512;
/// Longest text of one window; a 30 s clause of fast Chinese stays below it.
const MAX_NEW_TOKENS: i32 = 256;

pub struct Qwen3Asr {
    recognizer: OfflineRecognizer,
}

impl Qwen3Asr {
    /// `dir` holds the ONNX files and the tokenizer files of the manifest entry.
    pub fn new(dir: impl AsRef<Path>, settings: &AsrConfig) -> Result<Self> {
        let dir = dir.as_ref();
        let conv_frontend = dir.join("conv_frontend.onnx");
        let encoder = dir.join("encoder.int8.onnx");
        let decoder = dir.join("decoder.int8.onnx");
        prepare(
            NAME,
            None,
            &[
                &conv_frontend,
                &encoder,
                &decoder,
                &dir.join("vocab.json"),
                &dir.join("merges.txt"),
                &dir.join("tokenizer_config.json"),
            ],
        )?;
        let conv_frontend = path_cstring(&conv_frontend, "Qwen3-ASR frontend path")?;
        let encoder = path_cstring(&encoder, "Qwen3-ASR encoder path")?;
        let decoder = path_cstring(&decoder, "Qwen3-ASR decoder path")?;
        let tokenizer = path_cstring(dir, "Qwen3-ASR tokenizer folder")?;
        let provider = cstring("cpu")?;
        let decoding = cstring("greedy_search")?;
        let hotwords = cstring("")?;
        let mut config = zeroed_config();
        // Whisper-style log-mel features.
        config.feat_config.feature_dim = 128;
        let qwen3 = &mut config.model_config.qwen3_asr;
        qwen3.conv_frontend = conv_frontend.as_ptr();
        qwen3.encoder = encoder.as_ptr();
        qwen3.decoder = decoder.as_ptr();
        qwen3.tokenizer = tokenizer.as_ptr();
        qwen3.max_total_len = MAX_TOTAL_TOKENS;
        qwen3.max_new_tokens = MAX_NEW_TOKENS;
        // sherpa samples with temperature and top-p; this temperature makes it greedy.
        qwen3.temperature = 1e-6;
        qwen3.top_p = 0.8;
        qwen3.seed = 42;
        qwen3.hotwords = hotwords.as_ptr();
        config.model_config.num_threads = settings.second_pass_engine_threads() as i32;
        config.model_config.provider = provider.as_ptr();
        config.decoding_method = decoding.as_ptr();
        // Every string above lives through this call.
        let recognizer = OfflineRecognizer::new(NAME, &config)?;
        Ok(Self { recognizer })
    }
}

impl OfflineAsr for Qwen3Asr {
    /// Qwen3-ASR detects the language itself and reports no tag. A forced language goes into
    /// its prompt, by name.
    fn decode(&mut self, samples: &[f32], language: Option<&str>) -> Result<OfflineText> {
        let language = language.map(|language| match language {
            "zh" => "Chinese",
            other => other,
        });
        self.recognizer.decode(samples, language)
    }
}
