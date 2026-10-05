//! Silero voice activity detection on sherpa's shared ONNX Runtime.
mod runtime;
mod silero;
pub use runtime::{default_runtime_path, initialize_shared_runtime, onnxruntime_version};
pub use silero::SileroVad;
