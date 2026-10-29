//! A GDI copy of the monitor under the cursor.
use lt_core::screen::Rect;
use std::time::Instant;
use windows::Win32::{
    Foundation::POINT,
    Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC,
        GetMonitorInfoW, MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, SRCCOPY,
    },
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        WindowsAndMessaging::GetCursorPos,
    },
};

/// One frozen monitor image.
pub struct Frame {
    pub id: u64,
    /// The monitor in virtual-desktop physical pixels.
    pub monitor: Rect,
    /// The monitor's DPI / 96, which is the page's device pixel ratio there.
    pub scale: f64,
    pub width: u32,
    pub height: u32,
    /// Top-down BGRA rows of `width * 4` bytes. The alpha bytes carry no meaning.
    pub bgra: Vec<u8>,
}

/// Releases the GDI objects in reverse order of creation.
#[derive(Default)]
struct Gdi {
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Drop for Gdi {
    fn drop(&mut self) {
        unsafe {
            if !self.previous.is_invalid() {
                SelectObject(self.memory, self.previous);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
            if !self.memory.is_invalid() {
                let _ = DeleteDC(self.memory);
            }
            if !self.screen.is_invalid() {
                ReleaseDC(None, self.screen);
            }
        }
    }
}

pub fn capture_at_cursor(id: u64) -> Result<Frame, String> {
    let started = Instant::now();
    let failed = |what: &str| format!("The screen could not be captured ({what})");
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.map_err(|_| failed("cursor"))?;
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(failed("monitor"));
    }
    let (mut dpi_x, mut dpi_y) = (96, 96);
    let scale =
        match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } {
            Ok(()) => f64::from(dpi_x) / 96.0,
            Err(_) => 1.0,
        };
    let area = info.rcMonitor;
    let (width, height) = (
        u32::try_from(area.right - area.left).map_err(|_| failed("size"))?,
        u32::try_from(area.bottom - area.top).map_err(|_| failed("size"))?,
    );

    let mut gdi = Gdi {
        screen: unsafe { GetDC(None) },
        ..Default::default()
    };
    if gdi.screen.is_invalid() {
        return Err(failed("screen"));
    }
    gdi.memory = unsafe { CreateCompatibleDC(Some(gdi.screen)) };
    if gdi.memory.is_invalid() {
        return Err(failed("memory"));
    }
    let header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // Negative: rows top to bottom.
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    gdi.bitmap = unsafe {
        CreateDIBSection(
            Some(gdi.memory),
            &header,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
    }
    .map_err(|_| failed("bitmap"))?;
    gdi.previous = unsafe { SelectObject(gdi.memory, gdi.bitmap.into()) };
    // CAPTUREBLT includes layered windows such as the caption overlay.
    unsafe {
        BitBlt(
            gdi.memory,
            0,
            0,
            width as i32,
            height as i32,
            Some(gdi.screen),
            area.left,
            area.top,
            SRCCOPY | CAPTUREBLT,
        )
    }
    .map_err(|_| failed("copy"))?;
    let _ = unsafe { GdiFlush() };
    let length = width as usize * height as usize * 4;
    // SAFETY: the DIB section holds `length` bytes and lives until `gdi` drops below.
    let bgra = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), length) }.to_vec();
    drop(gdi);
    tracing::debug!(
        ms = started.elapsed().as_millis() as u64,
        width,
        height,
        scale,
        "Screen captured"
    );
    Ok(Frame {
        id,
        monitor: Rect {
            x: area.left,
            y: area.top,
            w: width,
            h: height,
        },
        scale,
        width,
        height,
        bgra,
    })
}
