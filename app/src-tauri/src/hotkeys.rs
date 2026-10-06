//! Global hotkeys. A failed registration is reported to the control window; the others still work.
use crate::{app::Shared, controller::Cmd};
use lt_core::config::HotkeysConfig;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[derive(Clone, Copy)]
enum Action {
    MoveLock,
    ShowHide,
    Pause,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::MoveLock => "move_lock",
            Action::ShowHide => "show_hide",
            Action::Pause => "pause",
        }
    }
}

pub fn register(app: &AppHandle, hotkeys: &HotkeysConfig) {
    let shortcuts = app.global_shortcut();
    if let Err(error) = shortcuts.unregister_all() {
        tracing::warn!(%error, "Could not clear previous hotkeys");
    }
    let wanted = [
        (Action::MoveLock, &hotkeys.move_lock),
        (Action::ShowHide, &hotkeys.show_hide),
        (Action::Pause, &hotkeys.pause),
    ];
    for (action, accelerator) in wanted {
        if accelerator.trim().is_empty() {
            continue;
        }
        let result = shortcuts.on_shortcut(accelerator.as_str(), move |app, _, event| {
            if event.state == ShortcutState::Pressed {
                run(app, action);
            }
        });
        if let Err(error) = result {
            tracing::warn!(%error, action = action.name(), "Hotkey registration failed");
            let _ = app.emit_to(
                "control",
                "hotkeys://error",
                serde_json::json!({
                    "action": action.name(),
                    "accelerator": accelerator,
                    "message": error.to_string(),
                }),
            );
        }
    }
}

fn run(app: &AppHandle, action: Action) {
    let shared: Arc<Shared> = app.state::<Arc<Shared>>().inner().clone();
    match action {
        Action::MoveLock => {
            let moving = shared.state().overlay_moving;
            let _ = shared.set_overlay_moving(!moving);
        }
        Action::ShowHide => {
            let visible = shared.state().overlay_visible;
            let _ = shared.set_overlay_visible(!visible);
        }
        Action::Pause => {
            let _ = shared.controller.send(Cmd::TogglePause);
        }
    }
}
