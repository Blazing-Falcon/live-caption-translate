#[cfg(any(feature = "llm", feature = "sherpa"))]
use anyhow::Context;
use anyhow::{bail, Result};
use lt_core::{
    bus::EventBus,
    config::Config,
    engines::{SegmentAsr, Translator, Vad},
    fakes::{FakeAsr, FakeTranslator, FakeVad},
};
use std::path::PathBuf;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
};

pub struct Engines {
    pub vad: Box<dyn Vad>,
    pub asr: Box<dyn SegmentAsr>,
    pub translator: Box<dyn Translator>,
    pub pid: Arc<AtomicU32>,
    /// The draft translator (Continuous mode) and its server's process id.
    pub draft: Option<Box<dyn Translator>>,
    pub draft_pid: Arc<AtomicU32>,
    // These guards outlive the pipeline and kill the children on every exit path.
    #[cfg(feature = "llm")]
    pub supervisor: Option<lt_llm::supervisor::Supervisor>,
    #[cfg(feature = "llm")]
    pub draft_supervisor: Option<lt_llm::supervisor::Supervisor>,
}

pub fn config(path: Option<&Path>) -> Result<Config> {
    let environment_path = std::env::var_os("LT_CONFIG")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let path = path.or(environment_path.as_deref());
    let (config, messages) =
        path.map_or_else(|| Ok((Config::default(), Vec::new())), Config::load)?;
    for message in messages {
        eprintln!("Config: {message}");
    }
    Ok(config)
}

/// The parsed override already gives explicit CLI arguments precedence over
/// LT_MODELS_DIR. CLI development falls back to the workspace models folder.
pub fn models_dir(override_dir: Option<&Path>, config: &Config) -> PathBuf {
    override_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        if config.models.dir.is_empty() {
            PathBuf::from("models")
        } else {
            PathBuf::from(&config.models.dir)
        }
    })
}

pub fn check_cancel(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        return Err(lt_core::error::Error::Stopped.into());
    }
    Ok(())
}

pub fn build(
    config: &Config,
    models: &Path,
    binary: Option<&Path>,
    bus: &EventBus,
    fake: bool,
    cancelled: &AtomicBool,
) -> Result<Engines> {
    check_cancel(cancelled)?;
    if fake {
        return Ok(Engines {
            vad: Box::new(FakeVad::from_energy(0.001)),
            asr: Box::new(FakeAsr::new(vec![
                "你好，这是测试语音。".into();
                4096
            ])),
            translator: Box::<FakeTranslator>::default(),
            pid: Arc::new(AtomicU32::new(0)),
            draft: None,
            draft_pid: Arc::new(AtomicU32::new(0)),
            #[cfg(feature = "llm")]
            supervisor: None,
            #[cfg(feature = "llm")]
            draft_supervisor: None,
        });
    }
    #[cfg(all(feature = "sherpa", feature = "llm"))]
    {
        let mut registry = lt_core::engines::EngineRegistry::default();
        lt_sherpa::register_engines(&mut registry);
        let mut engine_config = config.clone();
        engine_config.models.dir = models.to_string_lossy().into_owned();
        let vad = registry.build_vad(&engine_config)?;
        let asr = registry.build_asr(&engine_config)?;
        let (translator, supervisor, pid) = translator(config, models, binary, bus, cancelled)?;
        let (draft, draft_supervisor, draft_pid) =
            match draft(config, models, binary, bus, cancelled)? {
                Some((draft, supervisor, pid)) => (Some(draft), supervisor, pid),
                None => (None, None, Arc::new(AtomicU32::new(0))),
            };
        Ok(Engines {
            vad,
            asr,
            translator,
            supervisor,
            pid,
            draft,
            draft_pid,
            draft_supervisor,
        })
    }
    #[cfg(not(all(feature = "sherpa", feature = "llm")))]
    {
        let _ = (config, models, binary, bus);
        bail!("Real replay needs engine features: cargo run -p lt-cli --features sherpa,llm -- replay ... (or --fake)");
    }
}

#[cfg(feature = "llm")]
pub type TranslationEngine = (
    Box<dyn Translator>,
    Option<lt_llm::supervisor::Supervisor>,
    Arc<AtomicU32>,
);

#[cfg(feature = "llm")]
pub fn translator(
    config: &Config,
    models: &Path,
    binary: Option<&Path>,
    bus: &EventBus,
    cancelled: &AtomicBool,
) -> Result<TranslationEngine> {
    check_cancel(cancelled)?;
    if config.translate.engine != "hymt2" {
        bail!("Unknown translator engine: {}", config.translate.engine);
    }
    if !config.translate.server_url.is_empty() {
        let mut registry = lt_core::engines::EngineRegistry::default();
        lt_llm::register_engines(&mut registry);
        return Ok((
            registry.build_translator(config)?,
            None,
            Arc::new(AtomicU32::new(0)),
        ));
    }
    let binary = binary.map(Path::to_path_buf).unwrap_or_else(default_binary);
    let options = lt_llm::supervisor::ServerOptions::new(
        binary,
        models.join("hy-mt2-1.8b-q4_0/Hy-MT2-1.8B.i1-Q4_0.gguf"),
        &config.translate,
        num_cpus::get_physical(),
    )
    // Off mode: no priority change.
    .with_below_normal(config.latency.low_priority && config.latency.mode != "off");
    let supervisor = lt_llm::supervisor::Supervisor::start(options, bus.clone())?;
    supervisor
        .wait_ready_cancellable(std::time::Duration::from_secs(65), cancelled)
        .context("Starting local translator")?;
    let mut registry = lt_core::engines::EngineRegistry::default();
    lt_llm::register_engines(&mut registry);
    let mut engine_config = config.clone();
    engine_config.translate.server_url = supervisor.url().into();
    let translator = registry.build_translator(&engine_config)?;
    let pid = supervisor.child_pid();
    Ok((translator, Some(supervisor), pid))
}

#[cfg(feature = "llm")]
pub type DraftEngine = (
    Box<dyn Translator>,
    Option<lt_llm::supervisor::Supervisor>,
    Arc<AtomicU32>,
);

/// The draft translator for Continuous mode, or `None` when the mode does not use one or the
/// model is not on disk (the pipeline then runs Light). `LT_DRAFT_MODEL` points at a GGUF
/// outside the models folder.
#[cfg(feature = "llm")]
pub fn draft(
    config: &Config,
    models: &Path,
    binary: Option<&Path>,
    bus: &EventBus,
    cancelled: &AtomicBool,
) -> Result<Option<DraftEngine>> {
    check_cancel(cancelled)?;
    let wanted = match config.latency.mode.as_str() {
        "continuous" => true,
        "auto" => num_cpus::get_physical() >= config.latency.auto_min_cores as usize,
        _ => false,
    };
    if !wanted {
        return Ok(None);
    }
    let mut registry = lt_core::engines::EngineRegistry::default();
    lt_llm::register_engines(&mut registry);
    if !config.latency.draft_server_url.is_empty() {
        return Ok(Some((
            registry.build_draft_translator(config)?,
            None,
            Arc::new(AtomicU32::new(0)),
        )));
    }
    let model = std::env::var_os("LT_DRAFT_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|| models.join("lmt-60-0.6b-q4_k_m/LMT-60-0.6B.Q4_K_M.gguf"));
    if !model.is_file() {
        eprintln!(
            "Draft model not found at {}; running without drafts",
            model.display()
        );
        return Ok(None);
    }
    let binary = binary.map(Path::to_path_buf).unwrap_or_else(default_binary);
    let options = lt_llm::supervisor::ServerOptions::new(
        binary,
        model,
        &config.translate,
        num_cpus::get_physical(),
    )
    .with_role(lt_llm::supervisor::ServerRole::Draft)
    .with_below_normal(config.latency.low_priority);
    let supervisor = lt_llm::supervisor::Supervisor::start(options, bus.clone())?;
    supervisor
        .wait_ready_cancellable(std::time::Duration::from_secs(65), cancelled)
        .context("Starting draft translator")?;
    let mut engine_config = config.clone();
    engine_config.latency.draft_server_url = supervisor.url().into();
    let translator = registry.build_draft_translator(&engine_config)?;
    let pid = supervisor.child_pid();
    Ok(Some((translator, Some(supervisor), pid)))
}

#[cfg(feature = "llm")]
fn default_binary() -> PathBuf {
    let name = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(name)))
        .unwrap_or_else(|| name.into())
}

#[cfg(feature = "sherpa")]
pub fn wav_files(input: &Path) -> Result<Vec<PathBuf>> {
    if input.is_file() {
        return Ok(vec![input.to_path_buf()]);
    }
    let mut files: Vec<_> = std::fs::read_dir(input)
        .with_context(|| format!("Reading {}", input.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        .collect();
    files.sort();
    if files.is_empty() {
        bail!("No WAV files in {}", input.display());
    }
    Ok(files)
}
