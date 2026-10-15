//! Engine construction for a listening session.
use lt_core::{
    bus::EventBus,
    config::Config,
    engines::{EngineRegistry, OfflineAsr, StreamingAsr, Translator},
    events::{EngineKind, EngineState, PipelineEvent},
};
use lt_llm::supervisor::{ServerOptions, Supervisor};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32},
        Arc,
    },
    time::Duration,
};

/// The live recognizer, the second-pass recognizer unless it is off, and the pid of the
/// second-pass server (zero when it is off).
pub type Speech = (
    Box<dyn StreamingAsr>,
    Option<Box<dyn OfflineAsr>>,
    Arc<AtomicU32>,
);

/// Qwen3-ASR runs in its own llama-server (`binary`); quitting (`cancelled`) stops its start.
pub fn speech(
    config: &Config,
    models: &Path,
    binary: &Path,
    cancelled: &AtomicBool,
) -> Result<Speech, String> {
    let mut registry = EngineRegistry::default();
    lt_sherpa::register_engines(&mut registry);
    let mut engine_config = config.clone();
    engine_config.models.dir = models.to_string_lossy().into_owned();
    let asr = registry
        .build_asr(&engine_config)
        .map_err(|e| e.to_string())?;
    if config.asr.second_pass != "qwen3" {
        return Ok((asr, None, Arc::default()));
    }
    let server = lt_llm::speech::Qwen3Server::start(
        binary,
        &models.join(lt_llm::speech::QWEN3_MODEL),
        &config.asr,
        config.performance.low_priority,
        cancelled,
    )
    .map_err(|e| format!("Second pass: {e}"))?;
    let pid = server.child_pid();
    Ok((asr, Some(Box::new(server)), pid))
}

pub fn fail(bus: &EventBus, engine: EngineKind, message: &str) {
    bus.publish(PipelineEvent::EngineStatus {
        engine,
        state: EngineState::Failed,
        message: Some(message.into()),
    });
}

/// Starts the bundled llama-server, which outlives pipeline restarts.
pub fn start_supervisor(
    config: &Config,
    models: &Path,
    binary: &Path,
    bus: &EventBus,
    cancelled: &AtomicBool,
) -> Result<Supervisor, String> {
    let (id, file) = lt_core::config::TranslateConfig::MODEL;
    let options = ServerOptions::new(
        binary.to_path_buf(),
        models.join(id).join(file),
        &config.translate,
    )
    .with_below_normal(config.performance.low_priority);
    let supervisor = Supervisor::start(options, bus.clone()).map_err(|e| e.to_string())?;
    supervisor
        .wait_ready_cancellable(Duration::from_secs(65), cancelled)
        .map_err(|e| format!("The translator did not start: {e}"))?;
    Ok(supervisor)
}

/// A client for either the supervised server or the configured external URL.
pub fn translator(
    config: &Config,
    supervisor: Option<&Supervisor>,
) -> Result<(Box<dyn Translator>, Arc<AtomicU32>), String> {
    let mut registry = EngineRegistry::default();
    lt_llm::register_engines(&mut registry);
    let mut engine_config = config.clone();
    let pid = match supervisor {
        Some(supervisor) => {
            engine_config.translate.server_url = supervisor.url().into();
            supervisor.child_pid()
        }
        None => Arc::new(AtomicU32::new(0)),
    };
    let translator = registry
        .build_translator(&engine_config)
        .map_err(|e| e.to_string())?;
    Ok((translator, pid))
}
