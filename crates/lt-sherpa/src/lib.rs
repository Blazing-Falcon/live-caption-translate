//! sherpa-onnx recognizers (SenseVoice, X-ASR, Fun-ASR-Nano, Qwen3-ASR) and the voice
//! activity engines (Silero v5 and v6, TEN VAD) that run through ort.

mod llm_asr;
mod offline;
mod runtime;
mod sensevoice;
mod silero;
mod ten;
mod xasr;
mod xasr_stream;

pub use llm_asr::{FunAsrNano, Qwen3Asr};
pub use runtime::{
    default_runtime_path, initialize_shared_runtime, onnxruntime_version, verify_sensevoice_runtime,
};
pub use sensevoice::SenseVoiceAsr;
pub use silero::SileroVad;
pub use ten::TenVad;
pub use xasr::XAsr;
pub use xasr_stream::XAsrStream;

/// Manifest folders of the speech models.
pub const XASR_MODEL: &str = "xasr-zh-en-int8-2026-06-03";
pub const FUNASR_NANO_MODEL: &str = "funasr-nano-int8-2025-12-30";
pub const QWEN3_ASR_MODEL: &str = "qwen3-asr-0.6b-int8-2026-03-25";

/// The streaming X-ASR model for `asr.stream_chunk_ms`.
pub fn xasr_stream_model(chunk_ms: u32) -> &'static str {
    match chunk_ms {
        160 => "xasr-stream-160ms-int8-2026-06-05",
        960 => "xasr-stream-960ms-int8-2026-06-05",
        _ => "xasr-stream-480ms-int8-2026-06-05",
    }
}

/// The manifest model ids a configuration needs for its VAD, live recognizer and second pass.
pub fn model_ids(config: &lt_core::config::Config) -> Vec<&'static str> {
    let vad = match config.vad.engine.as_str() {
        "silero_v6" => "silero-vad-v6",
        "ten" => "ten-vad",
        _ => "silero-vad-v5",
    };
    let live = match config.asr.engine.as_str() {
        "xasr" => XASR_MODEL,
        "sensevoice" => "sensevoice-2024-07-17-int8",
        _ => xasr_stream_model(config.asr.stream_chunk_ms),
    };
    let mut ids = vec![vad, live];
    ids.extend(second_pass_model(&config.asr.second_pass));
    ids
}

fn second_pass_model(engine: &str) -> Option<&'static str> {
    match engine {
        "xasr" => Some(XASR_MODEL),
        "funasr_nano" => Some(FUNASR_NANO_MODEL),
        "qwen3_asr" => Some(QWEN3_ASR_MODEL),
        _ => None,
    }
}

/// Native constructors remain selected by the platform-free core registry.
pub fn register_engines(registry: &mut lt_core::engines::EngineRegistry) {
    registry.register_vad("silero", |config| {
        let models = models_dir(config);
        Ok(Box::new(SileroVad::new(
            models.join("silero-vad-v5/silero_vad_v5.onnx"),
        )?))
    });
    registry.register_vad("silero_v6", |config| {
        let models = models_dir(config);
        Ok(Box::new(SileroVad::new(
            models.join("silero-vad-v6/silero_vad_v6.onnx"),
        )?))
    });
    registry.register_vad("ten", |config| {
        let models = models_dir(config);
        Ok(Box::new(TenVad::new(models.join("ten-vad/ten-vad.onnx"))?))
    });
    registry.register_asr("sensevoice", |config| {
        let models = models_dir(config);
        Ok(Box::new(SenseVoiceAsr::new(
            models.join("sensevoice-2024-07-17-int8/model.int8.onnx"),
            models.join("sensevoice-2024-07-17-int8/tokens.txt"),
            &config.asr,
        )?))
    });
    registry.register_asr("xasr_stream", |config| {
        Ok(Box::new(XAsrStream::new(
            models_dir(config).join(xasr_stream_model(config.asr.stream_chunk_ms)),
            &config.asr,
        )?))
    });
    registry.register_asr("xasr", |config| {
        Ok(Box::new(XAsr::new(
            models_dir(config).join(XASR_MODEL),
            config.asr.threads,
        )?))
    });
    registry.register_second_pass("xasr", |config| {
        Ok(Box::new(XAsr::new(
            models_dir(config).join(XASR_MODEL),
            config.asr.second_pass_threads,
        )?))
    });
    registry.register_second_pass("funasr_nano", |config| {
        Ok(Box::new(FunAsrNano::new(
            models_dir(config).join(FUNASR_NANO_MODEL),
            config.asr.second_pass_threads,
            config.asr.use_itn,
        )?))
    });
    registry.register_second_pass("qwen3_asr", |config| {
        Ok(Box::new(Qwen3Asr::new(
            models_dir(config).join(QWEN3_ASR_MODEL),
            config.asr.second_pass_threads,
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
