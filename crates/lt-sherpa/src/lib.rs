//! SenseVoice recognition and Silero voice activity engines.

mod runtime;
mod sensevoice;
mod silero;

pub use runtime::{
    default_runtime_path, initialize_shared_runtime, onnxruntime_version, verify_sensevoice_runtime,
};
pub use sensevoice::SenseVoiceAsr;
pub use silero::SileroVad;

/// Native constructors remain selected by the platform-free core registry.
pub fn register_engines(registry: &mut lt_core::engines::EngineRegistry) {
    registry.register_vad("silero", |config| {
        let models = models_dir(config);
        Ok(Box::new(SileroVad::new(
            models.join("silero-vad-v5/silero_vad_v5.onnx"),
        )?))
    });
    registry.register_asr("sensevoice", |config| {
        let models = models_dir(config);
        Ok(Box::new(SenseVoiceAsr::new(
            models.join("sensevoice-2024-07-17-int8/model.int8.onnx"),
            models.join("sensevoice-2024-07-17-int8/tokens.txt"),
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
