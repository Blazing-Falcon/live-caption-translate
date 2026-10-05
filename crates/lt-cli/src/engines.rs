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
    // This guard outlives the pipeline and kills the child on every exit path.
    #[cfg(feature = "llm")]
    pub supervisor: Option<lt_llm::supervisor::Supervisor>,
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
        let vad = registry.build_vad(&engine_config)?;
        let asr = registry.build_asr(&engine_config)?;
        let (translator, supervisor, pid) = translator(config, models, binary, bus, cancelled)?;
        Ok(Engines {
            vad,
            asr,
            translator,
            supervisor,
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
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_override_then_config_then_workspace_default() {
        let mut config = Config::default();
        assert_eq!(models_dir(None, &config), PathBuf::from("models"));
        config.models.dir = "configured".into();
        assert_eq!(models_dir(None, &config), PathBuf::from("configured"));
        assert_eq!(
            models_dir(Some(Path::new("override")), &config),
            PathBuf::from("override")
        );
    }
}
