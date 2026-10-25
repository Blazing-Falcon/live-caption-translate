#[cfg(any(feature = "llm", feature = "sherpa"))]
use anyhow::Context;
use anyhow::{bail, Result};
use lt_core::{
    bus::EventBus,
    config::{Config, Message},
    engines::{OfflineAsr, StreamingAsr, Translator},
    fakes::{FakeOffline, FakeStream, FakeTranslator},
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
    pub asr: Box<dyn StreamingAsr>,
    pub second_pass: Box<dyn OfflineAsr>,
    pub translator: Box<dyn Translator>,
    pub pid: Arc<AtomicU32>,
    // These guards outlive the pipeline and kill the children on every exit path.
    #[cfg(feature = "llm")]
    pub supervisor: Option<lt_llm::supervisor::Supervisor>,
}

pub fn config(path: Option<&Path>) -> Result<(Config, Vec<Message>)> {
    let environment_path = std::env::var_os("LT_CONFIG")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let path = path.or(environment_path.as_deref());
    match path {
        Some(path) => Ok(Config::load(path)?),
        None => Ok((Config::default(), Vec::new())),
    }
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
            asr: Box::new(FakeStream::new(
                vec!["你好，这是测试语音。".into(); 4096],
                f64::from(config.asr.pause_s),
            )),
            second_pass: Box::<FakeOffline>::default(),
            translator: Box::<FakeTranslator>::default(),
            pid: Arc::new(AtomicU32::new(0)),
            #[cfg(feature = "llm")]
            supervisor: None,
        });
    }
    #[cfg(all(feature = "sherpa", feature = "llm"))]
    {
        let mut registry = lt_core::engines::EngineRegistry::default();
        lt_sherpa::register_engines(&mut registry);
        let mut engine_config = config.clone();
        engine_config.models.dir = models.to_string_lossy().into_owned();
        let asr = registry.build_asr(&engine_config)?;
        let second_pass: Box<dyn OfflineAsr> =
            Box::new(lt_sherpa::xasr_offline(models, config).context("Starting the second pass")?);
        let (translator, supervisor, pid) = translator(config, models, binary, bus, cancelled)?;
        Ok(Engines {
            asr,
            second_pass,
            translator,
            supervisor: Some(supervisor),
            pid,
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
    lt_llm::supervisor::Supervisor,
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
    let binary = binary.map(Path::to_path_buf).unwrap_or_else(default_binary);
    let (id, file) = lt_core::config::TranslateConfig::MODEL;
    let options = lt_llm::supervisor::ServerOptions::new(
        binary,
        models.join(id).join(file),
        &config.translate,
    )
    .with_below_normal(config.performance.low_priority);
    let supervisor = lt_llm::supervisor::Supervisor::start(options, bus.clone())?;
    supervisor
        .wait_ready_cancellable(std::time::Duration::from_secs(65), cancelled)
        .context("Starting local translator")?;
    let translator = lt_llm::OpenAiCompatTranslator::new(supervisor.url())?;
    let pid = supervisor.child_pid();
    Ok((Box::new(translator), supervisor, pid))
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
