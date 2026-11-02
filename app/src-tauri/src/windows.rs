//! The control window, a normal native window that hides to the tray when closed, and the tray
//! panel, a small borderless window that opens above the tray icon and hides when it loses focus.
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

pub const CONTROL: &str = "control";
pub const TRAY: &str = "tray";
/// The tray panel's size in DIPs.
const TRAY_SIZE: (f64, f64) = (360.0, 440.0);

pub fn create_control(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, CONTROL, WebviewUrl::App("control.html".into()))
        .title("Live Translation")
        .inner_size(960.0, 680.0)
        .min_inner_size(780.0, 540.0)
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

pub fn create_tray_panel(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, TRAY, WebviewUrl::App("tray.html".into()))
        .title("Live Translation")
        .inner_size(TRAY_SIZE.0, TRAY_SIZE.1)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;
    Ok(())
}

/// Shows the panel above the click on the tray icon, inside that monitor's work area, or hides
/// it when it is open.
pub fn toggle_tray_panel(app: &AppHandle, click: PhysicalPosition<f64>) {
    if app.get_webview_window(TRAY).is_none() {
        if let Err(error) = create_tray_panel(app) {
            tracing::warn!(%error, "Could not create the tray panel");
            return;
        }
    }
    let Some(window) = app.get_webview_window(TRAY) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    if let Ok(Some(monitor)) = app.monitor_from_point(click.x, click.y) {
        let scale = monitor.scale_factor();
        let size = (
            (TRAY_SIZE.0 * scale).round() as i32,
            (TRAY_SIZE.1 * scale).round() as i32,
        );
        let area = monitor.work_area();
        let (x, y) = crate::geometry::tray_panel_origin(
            (click.x.round() as i32, click.y.round() as i32),
            size,
            (
                area.position.x,
                area.position.y,
                area.size.width as i32,
                area.size.height as i32,
            ),
        );
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    let _ = window.set_focus();
}

pub fn hide_tray_panel(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(TRAY) {
        let _ = window.hide();
    }
}
