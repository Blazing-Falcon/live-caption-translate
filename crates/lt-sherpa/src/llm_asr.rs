//! Fun-ASR-Nano and Qwen3-ASR: speech LLMs that sherpa decodes token by token. They run
//! only as the second pass, once per final clause. Both have an output cap: without one, a
//! short clip once decoded for minutes or looped on made-up text.

use crate::offline::{cstring, path_cstring, require_threads, zeroed_config, OfflineEngine};
use lt_core::{
    engines::{TokenTranscript, WindowAsr},
    error::Result,
};
use std::path::Path;

/// New tokens per clause for both models.
const MAX_NEW_TOKENS: i32 = 96;
/// Qwen3-ASR's whole sequence: prompt, audio and output.
const QWEN3_MAX_TOTAL_LEN: i32 = 512;

pub struct FunAsrNano {
    engine: OfflineEngine,
}

impl FunAsrNano {
    /// `dir` holds the three ONNX files and the tokenizer files of the manifest entry.
    pub fn new(dir: impl AsRef<Path>, threads: u32, use_itn: bool) -> Result<Self> {
        const NAME: &str = "Fun-ASR-Nano";
        let dir = dir.as_ref();
        require_threads(NAME, threads)?;
        let encoder_adaptor = dir.join("encoder_adaptor.int8.onnx");
        let llm = dir.join("llm.int8.onnx");
        let embedding = dir.join("embedding.int8.onnx");
        let tokenizer_files = ["vocab.json", "merges.txt", "tokenizer.json"].map(|f| dir.join(f));
        let mut files = vec![
            encoder_adaptor.as_path(),
            llm.as_path(),
            embedding.as_path(),
        ];
        files.extend(tokenizer_files.iter().map(|f| f.as_path()));
        OfflineEngine::prepare(NAME, None, &files)?;
        let encoder_adaptor = path_cstring(&encoder_adaptor, "Fun-ASR-Nano encoder path")?;
        let llm = path_cstring(&llm, "Fun-ASR-Nano LLM path")?;
        let embedding = path_cstring(&embedding, "Fun-ASR-Nano embedding path")?;
        let tokenizer = path_cstring(dir, "Fun-ASR-Nano tokenizer folder")?;
        let provider = cstring("cpu")?;
        let mut config = zeroed_config();
        let nano = &mut config.model_config.funasr_nano;
        nano.encoder_adaptor = encoder_adaptor.as_ptr();
        nano.llm = llm.as_ptr();
        nano.embedding = embedding.as_ptr();
        nano.tokenizer = tokenizer.as_ptr();
        nano.max_new_tokens = MAX_NEW_TOKENS;
        nano.itn = i32::from(use_itn);
        // Zero temperature, top_p and seed select sherpa's defaults.
        config.model_config.num_threads = threads as i32;
        config.model_config.provider = provider.as_ptr();
        Ok(Self {
            engine: OfflineEngine::create(NAME, &config)?,
        })
    }
}

impl WindowAsr for FunAsrNano {
    fn decode_window(&mut self, samples: &[f32]) -> Result<TokenTranscript> {
        Ok(self.engine.decode(samples)?.into_tokens())
    }
}

pub struct Qwen3Asr {
    engine: OfflineEngine,
}

impl Qwen3Asr {
    /// `dir` holds the three ONNX files and the tokenizer files of the manifest entry.
    pub fn new(dir: impl AsRef<Path>, threads: u32) -> Result<Self> {
        const NAME: &str = "Qwen3-ASR";
        let dir = dir.as_ref();
        require_threads(NAME, threads)?;
        let conv_frontend = dir.join("conv_frontend.onnx");
        let encoder = dir.join("encoder.int8.onnx");
        let decoder = dir.join("decoder.int8.onnx");
        let tokenizer_files =
            ["vocab.json", "merges.txt", "tokenizer_config.json"].map(|f| dir.join(f));
        let mut files = vec![
            conv_frontend.as_path(),
            encoder.as_path(),
            decoder.as_path(),
        ];
        files.extend(tokenizer_files.iter().map(|f| f.as_path()));
        OfflineEngine::prepare(NAME, None, &files)?;
        let conv_frontend = path_cstring(&conv_frontend, "Qwen3-ASR frontend path")?;
        let encoder = path_cstring(&encoder, "Qwen3-ASR encoder path")?;
        let decoder = path_cstring(&decoder, "Qwen3-ASR decoder path")?;
        let tokenizer = path_cstring(dir, "Qwen3-ASR tokenizer folder")?;
        let provider = cstring("cpu")?;
        let mut config = zeroed_config();
        let qwen = &mut config.model_config.qwen3_asr;
        qwen.conv_frontend = conv_frontend.as_ptr();
        qwen.encoder = encoder.as_ptr();
        qwen.decoder = decoder.as_ptr();
        qwen.tokenizer = tokenizer.as_ptr();
        qwen.max_total_len = QWEN3_MAX_TOTAL_LEN;
        qwen.max_new_tokens = MAX_NEW_TOKENS;
        // Qwen3-ASR takes 128 mel bins; the others take 80.
        config.feat_config.feature_dim = 128;
        config.model_config.num_threads = threads as i32;
        config.model_config.provider = provider.as_ptr();
        Ok(Self {
            engine: OfflineEngine::create(NAME, &config)?,
        })
    }
}

impl WindowAsr for Qwen3Asr {
    fn decode_window(&mut self, samples: &[f32]) -> Result<TokenTranscript> {
        Ok(self.engine.decode(samples)?.into_tokens())
    }
}
