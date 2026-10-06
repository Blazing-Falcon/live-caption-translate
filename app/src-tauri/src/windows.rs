//! The control window: a normal native window that hides to the tray when closed.
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const CONTROL: &str = "control";

pub fn create_control(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, CONTROL, WebviewUrl::App("control.html".into()))
        .title("Live Translation")
        .inner_size(900.0, 620.0)
        .min_inner_size(760.0, 520.0)
        .center()
        .visible(true)
        .build()?;
    Ok(())
}

/// Never focuses the overlay: only the control window comes forward.
pub fn show_control(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(CONTROL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    } else if let Err(error) = create_control(app) {
        tracing::warn!(%error, "Could not recreate the control window");
    }
}
