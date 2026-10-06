//! Tauri commands. Errors are short sentences a user can read.
use crate::{
    app::{SetConfigResult, Shared},
    controller::{self, Cmd},
    state::AppState,
};
use lt_audio::windows::{apps::AudioAppList, AudioDevice};
use lt_core::{config::Config, types::PipelineStats};
use lt_llm::models::ModelStatus;
use std::sync::Arc;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

type Shell<'a> = State<'a, Arc<Shared>>;
type Reply<T> = Result<T, String>;

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Reply<T> + Send + 'static) -> Reply<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("Internal error: {e}"))?
}

#[tauri::command]
pub fn get_state(shared: Shell) -> AppState {
    shared.state()
}

/// Config problems (shown once) plus hardware warnings, as plain sentences.
#[tauri::command]
pub fn startup_notices(shared: Shell) -> Vec<String> {
    let mut notices = std::mem::take(&mut *crate::app::lock(&shared.startup_messages));
    notices.extend(crate::system::current_warnings());
    notices
}

#[tauri::command]
pub async fn start_listening(shared: Shell<'_>) -> Reply<()> {
    let shared = shared.inner().clone();
    blocking(move || controller::request(&shared, Cmd::Start)).await
}

#[tauri::command]
pub async fn pause_listening(shared: Shell<'_>) -> Reply<()> {
    let shared = shared.inner().clone();
    blocking(move || controller::request(&shared, Cmd::Pause)).await
}

#[tauri::command]
pub fn get_config(shared: Shell) -> Config {
    shared.config()
}

#[tauri::command]
pub fn set_config(shared: Shell, patch: serde_json::Value) -> Reply<SetConfigResult> {
    shared.patch_config(patch)
}

#[tauri::command]
pub fn get_stats(shared: Shell) -> PipelineStats {
    crate::app::lock(&shared.stats).clone()
}

#[tauri::command]
pub async fn list_audio_devices() -> Reply<Vec<AudioDevice>> {
    blocking(|| lt_audio::windows::list_audio_devices().map_err(|e| e.to_string())).await
}

#[tauri::command]
pub async fn list_audio_apps(shared: Shell<'_>) -> Reply<AudioAppList> {
    let recent = shared.config().capture.apps;
    blocking(move || lt_audio::windows::apps::list_audio_apps(&recent).map_err(|e| e.to_string()))
        .await
}

#[tauri::command]
pub fn models_status(shared: Shell) -> Reply<Vec<ModelStatus>> {
    shared.models.status(&shared)
}

#[tauri::command]
pub fn models_download(shared: Shell, source: String) -> Reply<()> {
    shared.models.download(shared.inner(), &source)
}

#[tauri::command]
pub fn models_pause(shared: Shell) {
    shared.models.pause(&shared);
}

#[tauri::command]
pub async fn models_use_existing(shared: Shell<'_>, folder: String) -> Reply<Vec<ModelStatus>> {
    let shared = shared.inner().clone();
    blocking(move || shared.models.use_existing(&shared, &folder)).await
}

#[tauri::command]
pub fn set_overlay_moving(shared: Shell, moving: bool) -> Reply<()> {
    shared.set_overlay_moving(moving)
}

#[tauri::command]
pub fn set_overlay_visible(shared: Shell, visible: bool) -> Reply<()> {
    shared.set_overlay_visible(visible)
}

#[tauri::command]
pub fn open_folder(shared: Shell, which: String) -> Reply<()> {
    open_folder_native(&shared, &which)
}

/// Opens only the three fixed app folders, never a path supplied by the UI.
pub fn open_folder_native(shared: &Shared, which: &str) -> Reply<()> {
    let path = match which {
        "transcripts" => shared.paths.transcripts(),
        "logs" => shared.paths.logs(),
        "models" => shared.paths.models(&shared.config()),
        other => return Err(format!("Unknown folder: {other}")),
    };
    std::fs::create_dir_all(&path).map_err(|e| format!("Could not create the folder: {e}"))?;
    shared
        .handle
        .opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("Could not open the folder: {e}"))
}

/// The caption panel's Settings button needs the control window.
#[tauri::command]
pub fn show_control_window(shared: Shell) {
    shared.show_control();
}

#[tauri::command]
pub fn quit(shared: Shell) {
    crate::quit(shared.inner().clone());
}
