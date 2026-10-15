//! Speech recognition on sherpa-onnx's shared ONNX Runtime: streaming X-ASR for the live text,
//! and SenseVoice for the second pass. Qwen3-ASR runs in llama-server (`lt-llm`).

mod runtime;
mod sensevoice;
mod xasr_stream;

pub use runtime::{default_runtime_path, initialize_shared_runtime, onnxruntime_version};
pub use sensevoice::SenseVoice;
pub use xasr_stream::XAsrStream;

/// The SenseVoice model of the second pass.
pub const SENSEVOICE_MODEL: &str = "sensevoice-2024-07-17-int8";

/// The streaming X-ASR model for `asr.chunk_ms`.
pub fn xasr_stream_model(chunk_ms: u32) -> &'static str {
    match chunk_ms {
        160 => "xasr-stream-160ms-int8-2026-06-05",
        960 => "xasr-stream-960ms-int8-2026-06-05",
        _ => "xasr-stream-480ms-int8-2026-06-05",
    }
}

/// The manifest model ids a configuration needs for the recognizers in this crate.
pub fn model_ids(config: &lt_core::config::Config) -> Vec<&'static str> {
    let mut ids = vec![xasr_stream_model(config.asr.chunk_ms)];
    if config.asr.second_pass == "sensevoice" {
        ids.push(SENSEVOICE_MODEL);
    }
    ids
}

/// Native constructors remain selected by the platform-free core registry.
pub fn register_engines(registry: &mut lt_core::engines::EngineRegistry) {
    registry.register_asr(|config| {
        Ok(Box::new(XAsrStream::new(
            models_dir(config).join(xasr_stream_model(config.asr.chunk_ms)),
            &config.asr,
        )?))
    });
    registry.register_second_pass(|config| match config.asr.second_pass.as_str() {
        "sensevoice" => Ok(Some(Box::new(SenseVoice::new(
            models_dir(config).join(SENSEVOICE_MODEL),
            &config.asr,
        )?))),
        _ => Ok(None),
    });
}

fn models_dir(config: &lt_core::config::Config) -> std::path::PathBuf {
    if config.models.dir.is_empty() {
        "models".into()
    } else {
        config.models.dir.clone().into()
    }
}
