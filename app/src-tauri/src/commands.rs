//! Tauri commands. Errors are short sentences a user can read.
use crate::{
    app::{SetConfigResult, Shared},
    controller::{self, Cmd},
    state::AppState,
};
use lt_audio::windows::{apps::AudioAppList, AudioDevice};
use lt_core::{config::Config, screen::Rect, types::PipelineStats};
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
pub fn reset_config(shared: Shell) -> Reply<SetConfigResult> {
    shared.reset_config()
}

#[derive(serde::Serialize)]
pub struct TranslatorTest {
    text: String,
    ms: u64,
}

#[tauri::command]
pub async fn test_translator(shared: Shell<'_>) -> Reply<TranslatorTest> {
    let shared = shared.inner().clone();
    blocking(move || {
        let config = shared.config();
        let mut translator: Box<dyn lt_core::engines::Translator> =
            if config.translate.engine.is_local() {
                Box::new(
                    lt_llm::OpenAiCompatTranslator::new(&controller::server_url(&shared)?)
                        .map_err(|e| e.to_string())?,
                )
            } else {
                lt_web::build(&config.translate).map_err(|e| e.to_string())?
            };
        let started = std::time::Instant::now();
        let answer = translator
            .translate(
                &lt_core::engines::TranslateRequest {
                    id: lt_core::types::UtteranceId(0),
                    text: "今天的会议改到下午三点。",
                    max_tokens: None,
                    control: lt_core::engines::TranslationControl {
                        deadline: started + lt_core::engines::REQUEST_TIMEOUT,
                        cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        abort: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    },
                },
                &mut |_| {},
            )
            .map_err(|e| e.to_string())?;
        Ok(TranslatorTest {
            text: answer.text,
            ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        })
    })
    .await
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
pub fn models_download(shared: Shell) -> Reply<()> {
    shared.models.download(shared.inner(), None)
}

#[tauri::command]
pub fn models_download_optional(shared: Shell, id: String) -> Reply<()> {
    shared.models.download(shared.inner(), Some(id))
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

/// Opens the control window from the tray panel and closes the panel.
#[tauri::command]
pub fn open_settings(shared: Shell) {
    crate::windows::hide_tray_panel(&shared.handle);
    shared.show_control();
}

/// The saved transcripts, newest first.
#[tauri::command]
pub async fn list_transcripts(shared: Shell<'_>) -> Reply<Vec<lt_core::transcript::SavedSession>> {
    let dir = shared.paths.transcripts();
    blocking(move || {
        lt_core::transcript::saved_sessions(dir)
            .map_err(|e| format!("Could not read the transcripts: {e}"))
    })
    .await
}

/// The lines of one saved transcript, named as `list_transcripts` names it.
#[tauri::command]
pub async fn read_transcript(
    shared: Shell<'_>,
    file: String,
) -> Reply<Vec<lt_core::transcript::SavedLine>> {
    let dir = shared.paths.transcripts();
    blocking(move || {
        lt_core::transcript::saved_lines(dir, &file)
            .map_err(|e| format!("Could not read this transcript: {e}"))
    })
    .await
}

/// Starts reading and translating `rect` (frame pixels) of the frozen frame; returns the job id.
#[tauri::command]
pub async fn screen_select(
    shared: Shell<'_>,
    frame: u64,
    rect: Rect,
    debug: Option<bool>,
    direction: Option<lt_core::screen::Direction>,
) -> Reply<u64> {
    let shared = shared.inner().clone();
    blocking(move || {
        shared.screen.select(
            frame,
            rect,
            debug.unwrap_or(false),
            direction.unwrap_or_else(|| crate::app::lock(&shared.config).screen.direction),
        )
    })
    .await
}

#[tauri::command]
pub fn screen_close(shared: Shell) {
    crate::screen::close(&shared);
}

#[tauri::command]
pub async fn screen_plate(
    shared: Shell<'_>,
    request: crate::screen::PlateRequest,
) -> Reply<crate::screen::PlateReply> {
    let shared = shared.inner().clone();
    blocking(move || shared.screen.plate(request)).await
}

/// Stops listening and the server, then exits.
#[tauri::command]
pub fn quit(shared: Shell) {
    crate::quit(shared.inner().clone());
}
