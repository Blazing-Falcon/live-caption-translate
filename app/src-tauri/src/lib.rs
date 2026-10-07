//! Tauri shell: windows, tray, hotkeys, and the bridge between the pipeline and the UI.
#![cfg(windows)]

mod app;
mod applies;
mod bridge;
mod coalesce;
mod commands;
mod controller;
mod engines;
mod geometry;
mod hotkeys;
mod models;
mod overlay;
mod paths;
mod state;
mod system;
mod tray;
mod webview2;
mod windows;

use app::{lock, Shared};
use lt_core::{bus::EventBus, config::Config};
use std::{
    sync::{atomic::Ordering, Arc, Mutex, OnceLock},
    time::SystemTime,
};
use tauri::{Manager, RunEvent, WindowEvent};

static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

fn init_logging(paths: &paths::Paths, config: &Config) {
    let filter = std::env::var("LT_LOG").unwrap_or_else(|_| {
        let level = &config.logging.level;
        format!("warn,live_translation_lib={level},lt_core={level},lt_audio={level},lt_llm={level},lt_sherpa={level}")
    });
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("live-translation")
        .filename_suffix("log")
        .max_log_files(config.logging.keep_files.max(1) as usize)
        .build(paths.logs());
    let Ok(appender) = appender else {
        return;
    };
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = LOG_GUARD.set(guard);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(writer)
        .with_ansi(false)
        .try_init();
}

/// Explicit shutdown: stop the pipeline and translator first, then leave the process.
pub fn quit(shared: Arc<Shared>) {
    if shared.quitting.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        controller::shutdown(&shared);
        shared.handle.exit(0);
    });
}

pub fn run() {
    if !webview2::ensure_runtime() {
        return;
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            windows::show_control(app);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let paths = paths::Paths::resolve(&handle)?;
            let (config, messages) = Config::load(&paths.config_file).unwrap_or_else(|error| {
                (
                    Config::default(),
                    vec![format!("Settings were reset: {error}")],
                )
            });
            init_logging(&paths, &config);
            for message in &messages {
                tracing::warn!(message, "Config");
            }
            let models = paths.models(&config);
            let state = state::AppState {
                models_ready: models::all_ready(&models),
                overlay_visible: config.overlay.visible,
                ..Default::default()
            };
            let (controller_tx, controller_rx) = crossbeam_channel::unbounded();
            let retention = config.transcript.retention_days;
            let autostart = config.capture.autostart;
            let shared = Arc::new(Shared {
                handle: handle.clone(),
                paths: paths.clone(),
                bus: EventBus::default(),
                state: Mutex::new(state),
                config: Mutex::new(config.clone()),
                stats: Mutex::new(Default::default()),
                controller: controller_tx,
                models: Default::default(),
                overlay: Default::default(),
                tray: Mutex::new(None),
                quitting: Default::default(),
                startup_messages: Mutex::new(messages),
            });
            app.manage(shared.clone());
            controller::spawn(shared.clone(), controller_rx)?;
            bridge::spawn(shared.clone())?;
            windows::create_control(&handle)?;
            shared.overlay.create(&shared)?;
            overlay::spawn_hover_observer(shared.clone())?;
            *lock(&shared.tray) = Some(tray::build(&handle)?);
            shared.refresh_tray();
            hotkeys::register(&handle, &config.hotkeys);
            std::thread::spawn(move || {
                if retention > 0 {
                    let _ = lt_core::transcript::cleanup(
                        paths.transcripts(),
                        retention,
                        SystemTime::now(),
                    );
                }
            });
            if autostart && lock(&shared.state).models_ready {
                let shared = shared.clone();
                std::thread::spawn(move || {
                    if let Err(message) = controller::request(&shared, controller::Cmd::Start) {
                        tracing::warn!(message, "Autostart failed");
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != windows::CONTROL {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let quitting = window
                    .app_handle()
                    .try_state::<Arc<Shared>>()
                    .is_some_and(|shared| shared.quitting.load(Ordering::Acquire));
                if !quitting {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::startup_notices,
            commands::start_listening,
            commands::pause_listening,
            commands::get_config,
            commands::set_config,
            commands::get_stats,
            commands::list_audio_devices,
            commands::list_audio_apps,
            commands::models_status,
            commands::models_download,
            commands::models_pause,
            commands::models_use_existing,
            commands::set_overlay_moving,
            commands::set_overlay_visible,
            commands::open_folder,
            commands::show_control_window,
            commands::quit,
        ])
        .build(tauri::generate_context!())
        .expect("Live Translation could not start its windows");
    app.run(|app, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            let quitting = app
                .try_state::<Arc<Shared>>()
                .is_some_and(|shared| shared.quitting.load(Ordering::Acquire));
            // Closing the last window must keep the tray alive; only `quit` exits.
            if code.is_none() && !quitting {
                api.prevent_exit();
            }
        }
    });
}
