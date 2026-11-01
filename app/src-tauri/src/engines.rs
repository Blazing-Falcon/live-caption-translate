//! Engine construction for a listening session.
use lt_core::{
    bus::EventBus,
    config::Config,
    engines::{OfflineAsr, StreamingAsr, Translator},
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

type Recognizers = (Box<dyn StreamingAsr>, Box<dyn OfflineAsr>);

pub fn recognizers(config: &Config, models: &Path) -> Result<Recognizers, String> {
    let asr = lt_sherpa::XAsrStream::new(models.join(lt_sherpa::XASR_STREAM_MODEL), &config.asr)
        .map_err(|e| e.to_string())?;
    let second_pass =
        lt_sherpa::xasr_offline(models, config).map_err(|e| format!("Second pass: {e}"))?;
    Ok((Box::new(asr), Box::new(second_pass)))
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

/// A client for the server the supervisor owns.
pub fn translator(
    config: &Config,
    supervisor: Option<&Supervisor>,
) -> Result<(Box<dyn Translator>, Arc<AtomicU32>), String> {
    if !config.translate.engine.is_local() {
        return lt_web::build(&config.translate)
            .map(|translator| (translator, Arc::new(AtomicU32::new(0))))
            .map_err(|e| e.to_string());
    }
    let supervisor = supervisor.ok_or("Local translation server is not running")?;
    let translator =
        lt_llm::OpenAiCompatTranslator::new(supervisor.url()).map_err(|e| e.to_string())?;
    Ok((Box::new(translator), supervisor.child_pid()))
}
