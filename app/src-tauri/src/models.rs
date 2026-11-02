//! Model status, downloads and imports for the first-run screen.
use crate::app::{lock, Shared};
use lt_core::config::Config;
use lt_llm::models::{ModelManager, ModelState, ModelStatus};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tauri::Emitter;

/// The models listening needs with these settings, in download order.
pub fn required(config: &Config) -> Vec<&'static str> {
    let mut wanted = vec![lt_sherpa::XASR_STREAM_MODEL];
    if config.asr.second_pass {
        wanted.push(lt_sherpa::XASR_OFFLINE_MODEL);
    }
    if config.translate.engine.is_local() {
        wanted.push(lt_core::config::TranslateConfig::MODEL.0);
    }
    wanted
}

/// Every model needed to listen is on disk and verified.
pub fn all_ready(dir: &Path, config: &Config) -> bool {
    missing_for(dir, config).is_empty()
}

pub fn missing_for(dir: &Path, config: &Config) -> Vec<String> {
    let wanted = required(config);
    match lt_llm::models::status(dir) {
        Ok(models) => models
            .into_iter()
            .filter(|m| wanted.contains(&m.id.as_str()) && m.state != ModelState::Ready)
            .map(|m| m.name)
            .collect(),
        Err(_) => wanted.into_iter().map(String::from).collect(),
    }
}

/// The control window shows every download; the screen window shows the one it asked for.
fn emit(shared: &Shared, event: &str, payload: &impl serde::Serialize) {
    for window in [crate::windows::CONTROL, crate::screen::LABEL] {
        let _ = shared.handle.emit_to(window, event, payload);
    }
}

#[derive(Default)]
pub struct Models {
    /// Live progress for models being downloaded; disk state fills in the rest.
    live: Mutex<Vec<ModelStatus>>,
    cancel: Arc<AtomicBool>,
    busy: AtomicBool,
    /// The running download is one model, not the required set.
    busy_one: AtomicBool,
}

fn merge(disk: Vec<ModelStatus>, live: &[ModelStatus]) -> Vec<ModelStatus> {
    disk.into_iter()
        .map(|status| {
            live.iter()
                .find(|item| item.id == status.id)
                .filter(|item| {
                    matches!(
                        item.state,
                        ModelState::Downloading | ModelState::Verifying | ModelState::Paused
                    )
                })
                .cloned()
                .unwrap_or(status)
        })
        .collect()
}

impl Models {
    pub fn status(&self, shared: &Shared) -> Result<Vec<ModelStatus>, String> {
        let dir = shared.paths.models(&shared.config());
        let disk = ModelManager::new()
            .and_then(|manager| manager.status(dir))
            .map_err(|e| e.to_string())?;
        let mut statuses = merge(disk, &lock(&self.live));
        let wanted = required(&shared.config());
        for status in &mut statuses {
            if !wanted.contains(&status.id.as_str()) {
                status.optional = true;
            }
        }
        Ok(statuses)
    }

    pub fn publish(&self, shared: &Shared) {
        if let Ok(statuses) = self.status(shared) {
            let ready = statuses
                .iter()
                .filter(|m| !m.optional)
                .all(|m| m.state == ModelState::Ready);
            lock(&shared.state).models_ready = ready;
            emit(shared, "models://progress", &statuses);
        }
    }

    fn record(&self, shared: &Shared, status: ModelStatus) {
        {
            let mut live = lock(&self.live);
            match live.iter_mut().find(|item| item.id == status.id) {
                Some(item) => *item = status,
                None => live.push(status),
            }
        }
        self.publish(shared);
    }

    pub fn pause(&self, shared: &Shared) {
        self.cancel.store(true, Ordering::Release);
        {
            let mut live = lock(&self.live);
            for item in live.iter_mut() {
                if matches!(item.state, ModelState::Downloading | ModelState::Verifying) {
                    item.state = ModelState::Paused;
                }
            }
        }
        self.publish(shared);
    }

    /// Starts or resumes the missing models on a worker thread: the required ones, or only the
    /// model `only` names.
    pub fn download(&self, shared: &Arc<Shared>, only: Option<String>) -> Result<(), String> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return if only.is_none() && !self.busy_one.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err("A model download is already running. Try again when it ends.".into())
            };
        }
        self.busy_one.store(only.is_some(), Ordering::Release);
        self.cancel.store(false, Ordering::Release);
        let shared = shared.clone();
        let spawned = thread::Builder::new()
            .name("lt-models".into())
            .spawn(move || {
                let models = &shared.models;
                let outcome = ModelManager::new()
                    .map_err(|e| e.to_string())
                    .and_then(|manager| {
                        let config = shared.config();
                        let dir = shared.paths.models(&config);
                        let progress = &mut |status| models.record(&shared, status);
                        let ids = match &only {
                            None => required(&config),
                            Some(id) => vec![id.as_str()],
                        };
                        ids.into_iter()
                            .try_for_each(|id| {
                                manager
                                    .fetch_id(&dir, id, &models.cancel, progress)
                                    .map(drop)
                            })
                            .map_err(|e| e.to_string())
                    });
                if let Err(message) = outcome {
                    if !models.cancel.load(Ordering::Acquire) {
                        tracing::warn!(message, "Model download failed");
                        emit(
                            &shared,
                            "models://error",
                            &serde_json::json!({ "message": message }),
                        );
                        let mut live = lock(&models.live);
                        for item in live.iter_mut() {
                            if item.state == ModelState::Downloading {
                                item.state = ModelState::Paused;
                            }
                        }
                    }
                } else {
                    lock(&models.live).clear();
                }
                models.busy.store(false, Ordering::Release);
                models.publish(&shared);
            });
        if let Err(error) = spawned {
            self.busy.store(false, Ordering::Release);
            return Err(error.to_string());
        }
        Ok(())
    }

    /// Hash-verifies and copies matching files from a folder the user already has.
    pub fn use_existing(&self, shared: &Shared, folder: &str) -> Result<Vec<ModelStatus>, String> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err("A model download is already running. Pause it first.".into());
        }
        self.cancel.store(false, Ordering::Release);
        let result = ModelManager::new()
            .map_err(|e| e.to_string())
            .and_then(|manager| {
                let dir = shared.paths.models(&shared.config());
                manager
                    .use_existing(folder, dir, &self.cancel, &mut |status| {
                        self.record(shared, status);
                    })
                    .map_err(|e| e.to_string())
            });
        lock(&self.live).clear();
        self.busy.store(false, Ordering::Release);
        self.publish(shared);
        result?;
        self.status(shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_needs_only_the_models_its_settings_use() {
        let mut config = Config::default();
        assert_eq!(
            required(&config),
            [
                lt_sherpa::XASR_STREAM_MODEL,
                lt_sherpa::XASR_OFFLINE_MODEL,
                lt_core::config::TranslateConfig::MODEL.0,
            ]
        );
        config.asr.second_pass = false;
        config.translate.engine = lt_core::config::TranslateEngine::Microsoft;
        assert_eq!(required(&config), [lt_sherpa::XASR_STREAM_MODEL]);
    }
}
