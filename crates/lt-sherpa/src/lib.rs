//! Streaming X-ASR speech recognition on sherpa-onnx's shared ONNX Runtime. The second pass
//! (Qwen3-ASR) runs in llama-server (`lt-llm`).

mod runtime;
mod xasr_stream;

pub use runtime::{default_runtime_path, initialize_shared_runtime, onnxruntime_version};
pub use xasr_stream::XAsrStream;

/// The streaming X-ASR model for `asr.chunk_ms`.
pub fn xasr_stream_model(chunk_ms: u32) -> &'static str {
    match chunk_ms {
        160 => "xasr-stream-160ms-int8-2026-06-05",
        960 => "xasr-stream-960ms-int8-2026-06-05",
        _ => "xasr-stream-480ms-int8-2026-06-05",
    }
}


/// Native constructors remain selected by the platform-free core registry.
pub fn register_engines(registry: &mut lt_core::engines::EngineRegistry) {
    registry.register_asr(|config| {
        Ok(Box::new(XAsrStream::new(
            models_dir(config).join(xasr_stream_model(config.asr.chunk_ms)),
            &config.asr,
        )?))
    });
}

fn models_dir(config: &lt_core::config::Config) -> std::path::PathBuf {
    if config.models.dir.is_empty() {
        "models".into()
    } else {
        config.models.dir.clone().into()
    }
}
