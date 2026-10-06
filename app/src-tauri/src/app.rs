//! Process-wide shared state, managed by Tauri as `Arc<Shared>`.
use crate::{
    applies::{self, Applies},
    controller::Cmd,
    models::Models,
    overlay::Overlay,
    paths::Paths,
    state::AppState,
    tray::Tray,
};
use crossbeam_channel::Sender;
use lt_core::{bus::EventBus, config::Config, types::PipelineStats};
use serde::Serialize;
use std::sync::{atomic::AtomicBool, Mutex, MutexGuard};
use tauri::{AppHandle, Emitter};

#[derive(Serialize)]
pub struct SetConfigResult {
    pub config: Config,
    pub applied: Applies,
    pub messages: Vec<String>,
}

pub struct Shared {
    pub handle: AppHandle,
    pub paths: Paths,
    pub bus: EventBus,
    pub state: Mutex<AppState>,
    pub config: Mutex<Config>,
    pub stats: Mutex<PipelineStats>,
    pub controller: Sender<Cmd>,
    pub models: Models,
    pub overlay: Overlay,
    pub tray: Mutex<Option<Tray>>,
    pub quitting: AtomicBool,
    /// Config problems found at startup, handed to the control window once.
    pub startup_messages: Mutex<Vec<String>>,
}

/// A poisoned lock only means another thread panicked; the data is still usable.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Shared {
    pub fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    pub fn state(&self) -> AppState {
        lock(&self.state).clone()
    }

    /// Applies a patch, saves atomically, announces it, and tells the controller when
    /// capture or the pipeline must change. Returns the normalized config.
    pub fn patch_config(&self, patch: serde_json::Value) -> Result<SetConfigResult, String> {
        let (old, new, messages) = {
            let mut guard = lock(&self.config);
            let old = guard.clone();
            let mut new = old.clone();
            let messages = new.merge_patch(patch).map_err(|e| e.to_string())?;
            new.save(&self.paths.config_file)
                .map_err(|e| format!("Could not save settings: {e}"))?;
            *guard = new.clone();
            (old, new, messages)
        };
        let applied = applies::classify(&old, &new);
        let _ = self.handle.emit("config://changed", &new);
        self.on_config_changed(&old, &new);
        if matches!(applied, Applies::Capture | Applies::Pipeline) {
            self.controller
                .send(Cmd::Apply {
                    old,
                    new: new.clone(),
                })
                .map_err(|_| "The app is shutting down".to_string())?;
        }
        Ok(SetConfigResult {
            config: new,
            applied,
            messages,
        })
    }

    /// Updates a few fields from native code (saved overlay rects, visibility).
    pub fn update_config(&self, edit: impl FnOnce(&mut Config)) -> Result<(), String> {
        let new = {
            let mut guard = lock(&self.config);
            let mut next = guard.clone();
            edit(&mut next);
            next.save(&self.paths.config_file)
                .map_err(|e| format!("Could not save settings: {e}"))?;
            *guard = next.clone();
            next
        };
        let _ = self.handle.emit("config://changed", &new);
        Ok(())
    }

    fn on_config_changed(&self, old: &Config, new: &Config) {
        if old.hotkeys != new.hotkeys {
            crate::hotkeys::register(&self.handle, &new.hotkeys);
        }
        if old.overlay.visible != new.overlay.visible {
            self.apply_overlay_visible(new.overlay.visible);
        }
        if old.overlay.style != new.overlay.style
            || old.overlay.panel_edge != new.overlay.panel_edge
        {
            self.overlay.place(self);
        }
        if let Some(tray) = lock(&self.tray).as_ref() {
            tray.refresh(self);
        }
    }

    pub fn apply_overlay_visible(&self, visible: bool) {
        lock(&self.state).overlay_visible = visible;
        self.overlay.set_visible(self, visible);
        let _ = self.handle.emit(
            "overlay://visible",
            serde_json::json!({ "visible": visible }),
        );
    }

    pub fn set_overlay_visible(&self, visible: bool) -> Result<(), String> {
        self.update_config(|c| c.overlay.visible = visible)?;
        self.apply_overlay_visible(visible);
        self.refresh_tray();
        Ok(())
    }

    pub fn set_overlay_moving(&self, moving: bool) -> Result<(), String> {
        lock(&self.state).overlay_moving = moving;
        if moving && !self.config().overlay.visible {
            self.set_overlay_visible(true)?;
        }
        let result = self.overlay.set_moving(self, moving);
        let _ = self.handle.emit_to(
            "overlay",
            "overlay://mode",
            serde_json::json!({ "moving": moving }),
        );
        self.refresh_tray();
        result
    }

    pub fn refresh_tray(&self) {
        if let Some(tray) = lock(&self.tray).as_ref() {
            tray.refresh(self);
        }
    }

    pub fn show_control(&self) {
        crate::windows::show_control(&self.handle);
    }
}
