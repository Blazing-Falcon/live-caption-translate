//! Engine construction for a listening session.
use lt_core::{
    bus::EventBus,
    config::Config,
    engines::{EngineRegistry, SegmentAsr, Translator, Vad},
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

pub type SpeechEngines = (Box<dyn Vad>, Box<dyn SegmentAsr>);

pub const TRANSLATOR_MODEL: &str = "hy-mt2-1.8b-q4_0/Hy-MT2-1.8B.i1-Q4_0.gguf";

pub fn speech(config: &Config, models: &Path) -> Result<SpeechEngines, String> {
    let mut registry = EngineRegistry::default();
    lt_sherpa::register_engines(&mut registry);
    let mut engine_config = config.clone();
    engine_config.models.dir = models.to_string_lossy().into_owned();
    let vad = registry
        .build_vad(&engine_config)
        .map_err(|e| e.to_string())?;
    let asr = registry
        .build_asr(&engine_config)
        .map_err(|e| e.to_string())?;
    Ok((vad, asr))
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
    let options = ServerOptions::new(
        binary.to_path_buf(),
        models.join(TRANSLATOR_MODEL),
        &config.translate,
        num_cpus::get_physical(),
    );
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
