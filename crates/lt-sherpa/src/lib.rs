//! X-ASR speech recognition on sherpa-onnx's shared ONNX Runtime: the streaming recognizer and
//! its offline second pass.

mod runtime;
mod xasr_offline;
mod xasr_stream;

pub use runtime::{default_runtime_path, initialize_shared_runtime, onnxruntime_version};
pub use xasr_offline::XAsrOffline;
pub use xasr_stream::XAsrStream;

/// The manifest model of `asr.second_pass = "xasr"`.
pub const XASR_OFFLINE_MODEL: &str = "xasr-offline-int8-2026-06-03";

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

/// The offline X-ASR second pass, from `models` (the folder that holds every model).
pub fn xasr_offline(
    models: &std::path::Path,
    config: &lt_core::config::Config,
) -> lt_core::error::Result<XAsrOffline> {
    XAsrOffline::new(models.join(XASR_OFFLINE_MODEL), &config.asr)
}

fn models_dir(config: &lt_core::config::Config) -> std::path::PathBuf {
    if config.models.dir.is_empty() {
        "models".into()
    } else {
        config.models.dir.clone().into()
    }
}
