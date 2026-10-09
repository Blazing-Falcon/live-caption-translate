//! Model status, downloads and imports for the first-run screen.
use crate::app::{lock, Shared};
use lt_llm::models::{ModelManager, ModelSource, ModelState, ModelStatus};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};
use tauri::Emitter;

/// Every model needed to listen is on disk and verified (the draft model is optional).
pub fn all_ready(dir: &Path) -> bool {
    lt_llm::models::status(dir)
        .map(|models| {
            models
                .iter()
                .filter(|m| !m.optional)
                .all(|m| m.state == ModelState::Ready)
        })
        .unwrap_or(false)
}

#[derive(Default)]
pub struct Models {
    /// Live progress for models being downloaded; disk state fills in the rest.
    live: Mutex<Vec<ModelStatus>>,
    cancel: Arc<AtomicBool>,
    busy: AtomicBool,
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
            .and_then(|manager| manager.status_for_cores(dir, num_cpus::get_physical()))
            .map_err(|e| e.to_string())?;
        Ok(merge(disk, &lock(&self.live)))
    }

    fn publish(&self, shared: &Shared) {
        if let Ok(statuses) = self.status(shared) {
            let ready = statuses
                .iter()
                .filter(|m| !m.optional)
                .all(|m| m.state == ModelState::Ready);
            lock(&shared.state).models_ready = ready;
            let _ = shared
                .handle
                .emit_to("control", "models://progress", &statuses);
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

    /// Starts or resumes missing models on a worker thread. Without `ids` that is every required
    /// model plus the optional ones recommended for this PC (first run); with `ids`, exactly
    /// those (the Performance page's draft-model button).
    pub fn download(
        &self,
        shared: &Arc<Shared>,
        source: &str,
        ids: Option<Vec<String>>,
    ) -> Result<(), String> {
        let source = match source {
            "huggingface" => ModelSource::Huggingface,
            "modelscope" => ModelSource::Modelscope,
            other => return Err(format!("Unknown download source: {other}")),
        };
        if self.busy.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.cancel.store(false, Ordering::Release);
        let shared = shared.clone();
        let spawned = thread::Builder::new()
            .name("lt-models".into())
            .spawn(move || {
                let models = &shared.models;
                let outcome = ModelManager::new()
                    .map_err(|e| e.to_string())
                    .and_then(|manager| {
                        let dir = shared.paths.models(&shared.config());
                        let ids =
                            ids.unwrap_or_else(|| manager.default_ids(num_cpus::get_physical()));
                        manager
                            .fetch_ids(source, dir, &ids, &models.cancel, &mut |status| {
                                models.record(&shared, status);
                            })
                            .map_err(|e| e.to_string())
                    });
                if let Err(message) = outcome {
                    if !models.cancel.load(Ordering::Acquire) {
                        tracing::warn!(message, "Model download failed");
                        let _ = shared.handle.emit_to(
                            "control",
                            "models://error",
                            serde_json::json!({ "message": message }),
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

    fn status(id: &str, state: ModelState) -> ModelStatus {
        ModelStatus {
            id: id.into(),
            name: id.into(),
            bytes_total: 10,
            bytes_done: 5,
            state,
            optional: false,
            recommended: true,
        }
    }

    #[test]
    fn the_optional_draft_model_does_not_block_listening() {
        // all_ready consults the real manifest: with nothing on disk it is false, and an
        // optional model alone never decides the answer.
        assert!(!all_ready(Path::new("definitely/not/here")));
        let optional = ModelStatus {
            optional: true,
            ..status("draft", ModelState::Missing)
        };
        let required = status("asr", ModelState::Ready);
        let ready = [required, optional]
            .iter()
            .filter(|m| !m.optional)
            .all(|m| m.state == ModelState::Ready);
        assert!(ready);
    }
}
