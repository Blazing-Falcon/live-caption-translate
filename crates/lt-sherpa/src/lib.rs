//! Speech recognition on sherpa-onnx's shared ONNX Runtime: streaming X-ASR for the live text,
//! and SenseVoice or Qwen3-ASR for the second pass.

mod offline;
mod qwen3;
mod runtime;
mod sensevoice;
mod xasr_stream;

pub use qwen3::Qwen3Asr;
pub use runtime::{default_runtime_path, initialize_shared_runtime, onnxruntime_version};
pub use sensevoice::SenseVoice;
pub use xasr_stream::XAsrStream;

/// The second-pass models, by `asr.second_pass`.
pub const SENSEVOICE_MODEL: &str = "sensevoice-2024-07-17-int8";
pub const QWEN3_MODEL: &str = "qwen3-asr-0.6b-int8-2026-03-25";

/// The model of an `asr.second_pass` value; `None` when the second pass is off.
pub fn second_pass_model(second_pass: &str) -> Option<&'static str> {
    match second_pass {
        "sensevoice" => Some(SENSEVOICE_MODEL),
        "qwen3" => Some(QWEN3_MODEL),
        _ => None,
    }
}

/// The streaming X-ASR model for `asr.chunk_ms`.
pub fn xasr_stream_model(chunk_ms: u32) -> &'static str {
    match chunk_ms {
        160 => "xasr-stream-160ms-int8-2026-06-05",
        960 => "xasr-stream-960ms-int8-2026-06-05",
        _ => "xasr-stream-480ms-int8-2026-06-05",
    }
}

/// The manifest model ids a configuration needs for its recognizers.
pub fn model_ids(config: &lt_core::config::Config) -> Vec<&'static str> {
    let mut ids = vec![xasr_stream_model(config.asr.chunk_ms)];
    ids.extend(second_pass_model(&config.asr.second_pass));
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
    registry.register_second_pass(|config| {
        let Some(model) = second_pass_model(&config.asr.second_pass) else {
            return Ok(None);
        };
        let dir = models_dir(config).join(model);
        Ok(Some(if model == QWEN3_MODEL {
            Box::new(Qwen3Asr::new(dir, &config.asr)?)
        } else {
            Box::new(SenseVoice::new(dir, &config.asr)?)
        }))
    });
}

fn models_dir(config: &lt_core::config::Config) -> std::path::PathBuf {
    if config.models.dir.is_empty() {
        "models".into()
    } else {
        config.models.dir.clone().into()
    }
}
