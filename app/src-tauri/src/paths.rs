//! Data folders: config roams, large files stay local.
use lt_core::config::Config;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_file: PathBuf,
    pub local: PathBuf,
}

impl Paths {
    pub fn resolve(app: &AppHandle) -> Result<Self, String> {
        let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
        let local = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
        let config_file = std::env::var_os("LT_CONFIG")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| config_dir.join("config.toml"));
        Ok(Self { config_file, local })
    }

    pub fn models(&self, config: &Config) -> PathBuf {
        std::env::var_os("LT_MODELS_DIR")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| (!config.models.dir.is_empty()).then(|| PathBuf::from(&config.models.dir)))
            .unwrap_or_else(|| self.local.join("models"))
    }
    pub fn transcripts(&self) -> PathBuf {
        self.local.join("transcripts")
    }
    pub fn logs(&self) -> PathBuf {
        self.local.join("logs")
    }
}

/// The translator sidecar sits next to the executable; LT_LLAMA_SERVER overrides it for development.
pub fn llama_server() -> PathBuf {
    if let Some(path) = std::env::var_os("LT_LLAMA_SERVER").filter(|path| !path.is_empty()) {
        return path.into();
    }
    let name = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| Path::new(name).to_path_buf())
}
