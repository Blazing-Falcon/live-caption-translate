//! Screenshot translation: freeze the monitor under the cursor, then read and translate the box
//! the user drags. Every rect that leaves this module is in frame pixels.
mod bmp;
mod capture;
mod plates;
mod reuse;
mod worker;
pub use plates::{PlateReply, PlateRequest};

use crate::app::{lock, Shared};
use capture::Frame;
use lt_core::screen::Rect;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder,
};
pub use worker::spawn;

pub const LABEL: &str = "screen";

pub struct Screen {
    frame: Mutex<Option<Arc<Frame>>>,
    frames: AtomicU64,
    jobs: worker::Jobs,
}

impl Screen {
    pub fn new() -> (Self, crossbeam_channel::Receiver<worker::Work>) {
        let (jobs, work) = worker::Jobs::new();
        let screen = Self {
            frame: Mutex::default(),
            frames: AtomicU64::new(1),
            jobs,
        };
        (screen, work)
    }

    fn frame(&self) -> Option<Arc<Frame>> {
        lock(&self.frame).clone()
    }

    /// Cancels the running job and starts one on `rect` of frame `id`.
    pub fn select(&self, id: u64, rect: Rect, debug: bool) -> Result<u64, String> {
        let frame = self
            .frame()
            .filter(|frame| frame.id == id)
            .ok_or("This screenshot is closed.")?;
        let (crop, area) = crop(&frame, rect).ok_or("The selection is outside the screenshot.")?;
        Ok(self.jobs.start(id, crop, area, debug))
    }

    pub fn plate(&self, request: PlateRequest) -> Result<PlateReply, String> {
        self.jobs.plates.request(request)
    }
}

/// The part of `rect` inside the frame, as its own BGRA buffer, and where it was.
fn crop(frame: &Frame, rect: Rect) -> Option<(Vec<u8>, Rect)> {
    let (width, height) = (frame.width as i32, frame.height as i32);
    let (left, top) = (rect.x.clamp(0, width), rect.y.clamp(0, height));
    let (right, bottom) = (rect.right().clamp(0, width), rect.bottom().clamp(0, height));
    if right <= left || bottom <= top {
        return None;
    }
    let area = Rect {
        x: left,
        y: top,
        w: (right - left) as u32,
        h: (bottom - top) as u32,
    };
    let row = frame.width as usize * 4;
    let mut out = Vec::with_capacity(area.w as usize * area.h as usize * 4);
    for y in top..bottom {
        let start = y as usize * row + left as usize * 4;
        out.extend_from_slice(&frame.bgra[start..start + area.w as usize * 4]);
    }
    Some((out, area))
}

/// Created hidden at startup, so the hotkey only has to place and show it.
pub fn create_window(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("screen.html".into()))
        .title("Live Translation screenshot")
        .decorations(false)
        .transparent(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focusable(true)
        .resizable(false)
        .shadow(false)
        .visible(false)
        .build()?;
    Ok(())
}

/// The hotkey: freezes the monitor under the cursor, or closes the frozen view.
pub fn toggle(shared: &Arc<Shared>) {
    let Some(window) = shared.handle.get_webview_window(LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        close(shared);
        return;
    }
    // The capture comes before any window of ours changes, so none of them is in it.
    let id = shared.screen.frames.fetch_add(1, Ordering::Relaxed);
    let frame = match capture::capture_at_cursor(id) {
        Ok(frame) => frame,
        Err(message) => {
            tracing::warn!(message, "Screen capture failed");
            return;
        }
    };
    let (monitor, info) = (
        frame.monitor,
        serde_json::json!({
            "id": frame.id,
            "width": frame.width,
            "height": frame.height,
            "scale": frame.scale,
        }),
    );
    *lock(&shared.screen.frame) = Some(Arc::new(frame));
    // Position first: moving onto a monitor of another DPI rescales the window, and the size
    // set after that is the one that stays.
    let _ = window.set_position(PhysicalPosition::new(monitor.x, monitor.y));
    let _ = window.set_size(PhysicalSize::new(monitor.w, monitor.h));
    let _ = shared.handle.emit_to(LABEL, "screen://frame", info);
    let _ = window.show();
    let _ = window.set_focus();
    shared.screen.jobs.preload();
    let shared = shared.clone();
    std::thread::spawn(move || {
        if let Err(message) = crate::controller::server_url(&shared) {
            tracing::warn!(message, "Translation server did not start for a screenshot");
        }
    });
}

/// Cancels the job, hides the window and lets the frame go.
pub fn close(shared: &Shared) {
    shared.screen.jobs.cancel();
    if let Some(window) = shared.handle.get_webview_window(LABEL) {
        let _ = window.hide();
    }
    *lock(&shared.screen.frame) = None;
}

/// The `ltshot` protocol handler's answer for `path`.
pub fn frame_response(shared: Option<&Shared>, path: &str) -> tauri::http::Response<Vec<u8>> {
    let frame = shared.and_then(|shared| shared.screen.frame());
    bmp::response(frame.as_deref(), path)
}

pub fn plate_response(
    shared: Option<&Shared>,
    path: &str,
    query: Option<&str>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{header, Response, StatusCode};
    let builder = Response::builder().header(header::CACHE_CONTROL, "no-store");
    match shared.and_then(|s| s.screen.jobs.plates.image(path, query)) {
        Some(png) => builder.header(header::CONTENT_TYPE, "image/png").body(png),
        None => builder.status(StatusCode::NOT_FOUND).body(Vec::new()),
    }
    .expect("static headers are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_past_the_edge_is_cut_to_the_frame_with_its_origin() {
        let (width, height) = (5_u32, 4_u32);
        // Each pixel's blue byte is its index, so a wrong offset shows as wrong values.
        let bgra = (0..width * height)
            .flat_map(|index| [index as u8, 0, 0, 255])
            .collect();
        let frame = Frame {
            id: 1,
            monitor: Rect::default(),
            scale: 1.0,
            width,
            height,
            bgra,
        };
        let (pixels, area) = crop(
            &frame,
            Rect {
                x: 3,
                y: -2,
                w: 10,
                h: 4,
            },
        )
        .unwrap();
        assert_eq!(
            area,
            Rect {
                x: 3,
                y: 0,
                w: 2,
                h: 2
            }
        );
        let blue: Vec<u8> = pixels.chunks(4).map(|pixel| pixel[0]).collect();
        assert_eq!(blue, [3, 4, 8, 9]);
        assert!(crop(
            &frame,
            Rect {
                x: 6,
                y: 0,
                w: 3,
                h: 3
            }
        )
        .is_none());
    }
}
