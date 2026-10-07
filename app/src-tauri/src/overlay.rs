//! The caption overlay: a transparent, never-focused, click-through window.
use crate::{
    app::Shared,
    geometry::{self, Monitor, Physical},
};
use lt_core::config::{Config, OverlayRect};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

pub const LABEL: &str = "overlay";

#[derive(Default)]
pub struct Overlay {
    moving: AtomicBool,
}

impl Overlay {
    pub fn window(&self, app: &AppHandle) -> Option<WebviewWindow> {
        app.get_webview_window(LABEL)
    }

    /// Creates the window hidden, applies the native styles, then shows it without activation.
    pub fn create(&self, shared: &Shared) -> tauri::Result<()> {
        let app = &shared.handle;
        let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
            .title("Live Translation captions")
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .focusable(false)
            .shadow(false)
            .resizable(false)
            .visible(false)
            .build()?;
        apply_native_styles(&window);
        window.set_ignore_cursor_events(true)?;
        self.place(shared);
        if shared.config().overlay.visible {
            window.show()?;
        }
        Ok(())
    }

    pub fn set_visible(&self, shared: &Shared, visible: bool) {
        if let Some(window) = self.window(&shared.handle) {
            let _ = if visible {
                window.show()
            } else {
                window.hide()
            };
        }
    }

    /// Restores the saved rect for the current style, else the default placement.
    pub fn place(&self, shared: &Shared) {
        let Some(window) = self.window(&shared.handle) else {
            return;
        };
        let config = shared.config();
        let Some((monitors, primary)) = monitors(&window) else {
            return;
        };
        let panel = config.overlay.style == "panel";
        let saved = if panel {
            config.overlay.panel_rect.as_ref()
        } else {
            config.overlay.bar_rect.as_ref()
        };
        let name = saved.map_or("", |rect| rect.monitor.as_str());
        let Some(monitor) = geometry::pick(name, &monitors, primary) else {
            return;
        };
        // A rect saved for a vanished monitor is relative to that one; the default is safer.
        let rect = match saved {
            Some(rect) if rect.monitor == monitor.name => rect.clone(),
            _ => geometry::default_rect(panel, config.overlay.panel_edge != "left", monitor),
        };
        let target = geometry::to_physical(&rect, monitor);
        let _ = window.set_position(PhysicalPosition::new(target.x, target.y));
        let _ = window.set_size(PhysicalSize::new(target.w, target.h));
    }

    /// Move mode takes the click-through off; locking saves the rect for the current style.
    pub fn set_moving(&self, shared: &Shared, moving: bool) -> Result<(), String> {
        let was = self.moving.swap(moving, Ordering::AcqRel);
        let Some(window) = self.window(&shared.handle) else {
            return Ok(());
        };
        window
            .set_ignore_cursor_events(!moving)
            .map_err(|e| e.to_string())?;
        window.set_resizable(moving).map_err(|e| e.to_string())?;
        if was && !moving {
            self.save_rect(shared, &window)?;
        }
        Ok(())
    }

    fn save_rect(&self, shared: &Shared, window: &WebviewWindow) -> Result<(), String> {
        let (Some((monitors, primary)), Ok(position), Ok(size)) = (
            monitors(window),
            window.outer_position(),
            window.outer_size(),
        ) else {
            return Ok(());
        };
        let current = window
            .current_monitor()
            .ok()
            .flatten()
            .and_then(|m| m.name().cloned());
        let monitor = current
            .as_deref()
            .and_then(|name| geometry::pick(name, &monitors, primary))
            .or_else(|| monitors.get(primary));
        let Some(monitor) = monitor else {
            return Ok(());
        };
        let rect = geometry::from_physical(
            Physical {
                x: position.x,
                y: position.y,
                w: size.width,
                h: size.height,
            },
            monitor,
        );
        let panel = shared.config().overlay.style == "panel";
        shared.update_config(|config: &mut Config| {
            let slot: &mut Option<OverlayRect> = if panel {
                &mut config.overlay.panel_rect
            } else {
                &mut config.overlay.bar_rect
            };
            *slot = Some(rect);
        })
    }
}

fn monitors(window: &WebviewWindow) -> Option<(Vec<Monitor>, usize)> {
    let all = window.available_monitors().ok()?;
    let primary_name = window
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| m.name().cloned());
    let monitors: Vec<Monitor> = all
        .iter()
        .map(|monitor| {
            let area = monitor.work_area();
            Monitor {
                name: monitor.name().cloned().unwrap_or_default(),
                x: area.position.x,
                y: area.position.y,
                w: area.size.width,
                h: area.size.height,
                scale: monitor.scale_factor(),
            }
        })
        .collect();
    let primary = primary_name
        .and_then(|name| monitors.iter().position(|m| m.name == name))
        .unwrap_or(0);
    (!monitors.is_empty()).then_some((monitors, primary))
}

/// WS_EX_NOACTIVATE keeps focus on the video player; TOOLWINDOW hides it from Alt-Tab.
#[cfg(windows)]
fn apply_native_styles(window: &WebviewWindow) {
    use windows::Win32::{
        Foundation::SetLastError,
        UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_APPWINDOW,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    };
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        SetLastError(windows::Win32::Foundation::WIN32_ERROR(0));
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let wanted = (current | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize)
            & !(WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[cfg(not(windows))]
fn apply_native_styles(_: &WebviewWindow) {}

/// CSS `:hover` cannot fire through a click-through window, so the panel header is revealed by
/// observing the cursor natively. Only the panel style uses it; the page ignores it otherwise.
pub fn spawn_hover_observer(shared: std::sync::Arc<Shared>) -> std::io::Result<()> {
    use std::{sync::atomic::Ordering, time::Duration};
    use tauri::Emitter;
    std::thread::Builder::new()
        .name("lt-hover".into())
        .spawn(move || {
            let mut last = false;
            while !shared.quitting.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(100));
                let inside = shared.overlay.cursor_over_panel(&shared);
                if inside != last {
                    last = inside;
                    let _ = shared.handle.emit_to(
                        LABEL,
                        "overlay://hover",
                        serde_json::json!({ "hover": inside }),
                    );
                }
            }
        })
        .map(|_| ())
}

impl Overlay {
    fn cursor_over_panel(&self, shared: &Shared) -> bool {
        if self.moving.load(Ordering::Acquire) || shared.config().overlay.style != "panel" {
            return false;
        }
        let Some(window) = self.window(&shared.handle) else {
            return false;
        };
        let (Ok(cursor), Ok(position), Ok(size)) = (
            window.cursor_position(),
            window.outer_position(),
            window.outer_size(),
        ) else {
            return false;
        };
        cursor.x >= f64::from(position.x)
            && cursor.y >= f64::from(position.y)
            && cursor.x < f64::from(position.x) + f64::from(size.width)
            && cursor.y < f64::from(position.y) + f64::from(size.height)
    }
}
