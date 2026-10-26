//! X-ASR speech recognition on sherpa-onnx's shared ONNX Runtime: the streaming recognizer and
//! its offline second pass.

mod runtime;
mod xasr_offline;
mod xasr_stream;

pub use runtime::{default_runtime_path, initialize_shared_runtime, onnxruntime_version};
pub use xasr_offline::XAsrOffline;
pub use xasr_stream::XAsrStream;

/// The manifest model of the offline second pass.
pub const XASR_OFFLINE_MODEL: &str = "xasr-offline-int8-2026-06-03";

pub const XASR_STREAM_MODEL: &str = "xasr-stream-480ms-int8-2026-06-05";

/// The offline X-ASR second pass, from `models` (the folder that holds every model).
pub fn xasr_offline(
    models: &std::path::Path,
    config: &lt_core::config::Config,
) -> lt_core::error::Result<XAsrOffline> {
    XAsrOffline::new(models.join(XASR_OFFLINE_MODEL), &config.asr)
}
