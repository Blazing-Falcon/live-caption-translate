//! System tray menu. Status text is a disabled native item.
use crate::{
    app::{lock, Shared},
    controller::Cmd,
};
use lt_core::events::ListeningStateKind;
use std::sync::Arc;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

pub struct Tray {
    status: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    visible: MenuItem<tauri::Wry>,
    moving: MenuItem<tauri::Wry>,
    system: CheckMenuItem<tauri::Wry>,
    apps: CheckMenuItem<tauri::Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<Tray> {
    let status = MenuItem::with_id(app, "status", "Paused", false, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Start listening", true, None::<&str>)?;
    let visible = MenuItem::with_id(app, "visible", "Hide overlay", true, None::<&str>)?;
    let moving = MenuItem::with_id(app, "move", "Move overlay", true, None::<&str>)?;
    let system =
        CheckMenuItem::with_id(app, "mode_system", "Whole system", true, true, None::<&str>)?;
    let apps =
        CheckMenuItem::with_id(app, "mode_apps", "Selected apps", true, false, None::<&str>)?;
    let listen = Submenu::with_items(app, "Listen to", true, &[&system, &apps])?;
    let transcripts = MenuItem::with_id(
        app,
        "transcripts",
        "Open transcript folder",
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &separator,
            &pause,
            &visible,
            &moving,
            &listen,
            &transcripts,
            &settings,
            &separator,
            &quit,
        ],
    )?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or(tauri::Error::WindowNotFound)?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Live Translation")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(shared) = tray.app_handle().try_state::<Arc<Shared>>() {
                    shared.show_control();
                }
            }
        })
        .build(app)?;
    Ok(Tray {
        status,
        pause,
        visible,
        moving,
        system,
        apps,
    })
}

impl Tray {
    pub fn refresh(&self, shared: &Shared) {
        let state = shared.state();
        let config = shared.config();
        let lag = lock(&shared.stats).lag_ms;
        let label = state
            .source
            .as_ref()
            .map(|s| s.label.as_str())
            .unwrap_or("");
        let text = match state.listening {
            ListeningStateKind::Listening if lag > 0 => {
                format!("Listening · {label} · lag {:.1}s", lag as f64 / 1000.0)
            }
            ListeningStateKind::Listening => format!("Listening · {label}"),
            ListeningStateKind::Starting => "Starting…".to_string(),
            ListeningStateKind::Paused => "Paused".to_string(),
        };
        let hotkey = |key: &str| {
            if key.is_empty() {
                String::new()
            } else {
                format!("   {key}")
            }
        };
        let _ = self.status.set_text(text.trim_end_matches(" · "));
        let _ = self.pause.set_text(format!(
            "{}{}",
            if state.listening == ListeningStateKind::Listening {
                "Pause"
            } else {
                "Start listening"
            },
            hotkey(&config.hotkeys.pause)
        ));
        let _ = self.visible.set_text(format!(
            "{}{}",
            if state.overlay_visible {
                "Hide overlay"
            } else {
                "Show overlay"
            },
            hotkey(&config.hotkeys.show_hide)
        ));
        let _ = self.moving.set_text(format!(
            "{}{}",
            if state.overlay_moving {
                "Lock overlay"
            } else {
                "Move overlay"
            },
            hotkey(&config.hotkeys.move_lock)
        ));
        let _ = self.system.set_checked(config.capture.mode != "apps");
        let _ = self.apps.set_checked(config.capture.mode == "apps");
    }
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let shared: Arc<Shared> = app.state::<Arc<Shared>>().inner().clone();
    match event.id().as_ref() {
        "pause" => {
            let _ = shared.controller.send(Cmd::TogglePause);
        }
        "visible" => {
            let visible = shared.state().overlay_visible;
            let _ = shared.set_overlay_visible(!visible);
        }
        "move" => {
            let moving = shared.state().overlay_moving;
            let _ = shared.set_overlay_moving(!moving);
        }
        "mode_system" | "mode_apps" => {
            let mode = if event.id().as_ref() == "mode_apps" {
                "apps"
            } else {
                "system"
            };
            if let Err(message) =
                shared.patch_config(serde_json::json!({ "capture": { "mode": mode } }))
            {
                tracing::warn!(message, "Could not switch capture mode");
            }
            shared.refresh_tray();
        }
        "transcripts" => {
            let _ = crate::commands::open_folder_native(&shared, "transcripts");
        }
        "settings" => shared.show_control(),
        "quit" => crate::quit(shared),
        _ => {}
    }
}
