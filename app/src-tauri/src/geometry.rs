//! Overlay placement in device-independent pixels, relative to a monitor's work area.
//! Saving relative to the monitor keeps mixed-DPI desktops from dividing the whole
//! desktop origin by an unrelated scale factor.
use lt_core::config::OverlayRect;

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: String,
    /// Work area in physical pixels.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Physical {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

const MIN_W: f64 = 240.0;
const MIN_H: f64 = 80.0;

fn dip(value: u32, scale: f64) -> f64 {
    f64::from(value) / scale
}

/// Default placement: bar bottom-centered (max 960 wide), panel docked to an edge.
pub fn default_rect(panel: bool, edge_right: bool, monitor: &Monitor) -> OverlayRect {
    let (work_w, work_h) = (dip(monitor.w, monitor.scale), dip(monitor.h, monitor.scale));
    let (x, y, w, h) = if panel {
        let w = 420.0_f64.min(work_w - 48.0).max(MIN_W);
        let h = 600.0_f64.min(work_h - 48.0).max(MIN_H);
        let x = if edge_right { work_w - w - 24.0 } else { 24.0 };
        (x, 24.0, w, h)
    } else {
        let w = 960.0_f64.min(work_w * 0.9).max(MIN_W);
        // Two utterances with source lines plus the status chip row need about 190 DIPs.
        let h = 200.0;
        ((work_w - w) / 2.0, work_h - h - 40.0, w, h)
    };
    OverlayRect {
        monitor: monitor.name.clone(),
        x: x.round(),
        y: y.round(),
        w: w.round(),
        h: h.round(),
        ..OverlayRect::default()
    }
}

/// Saved rect to a physical window rect. The window is clamped inside the work area so a
/// changed resolution can never leave it unreachable.
pub fn to_physical(rect: &OverlayRect, monitor: &Monitor) -> Physical {
    let max_w = dip(monitor.w, monitor.scale).max(MIN_W);
    let max_h = dip(monitor.h, monitor.scale).max(MIN_H);
    let w = rect.w.clamp(MIN_W, max_w);
    let h = rect.h.clamp(MIN_H, max_h);
    let x = rect.x.clamp(0.0, (max_w - w).max(0.0));
    let y = rect.y.clamp(0.0, (max_h - h).max(0.0));
    Physical {
        x: monitor.x + (x * monitor.scale).round() as i32,
        y: monitor.y + (y * monitor.scale).round() as i32,
        w: (w * monitor.scale).round() as u32,
        h: (h * monitor.scale).round() as u32,
    }
}

pub fn from_physical(window: Physical, monitor: &Monitor) -> OverlayRect {
    OverlayRect {
        monitor: monitor.name.clone(),
        x: (f64::from(window.x - monitor.x) / monitor.scale).round(),
        y: (f64::from(window.y - monitor.y) / monitor.scale).round(),
        w: dip(window.w, monitor.scale).round(),
        h: dip(window.h, monitor.scale).round(),
        ..OverlayRect::default()
    }
}

/// Chooses the saved monitor by name, else the primary one (the monitor is gone).
pub fn pick<'a>(saved: &str, monitors: &'a [Monitor], primary: usize) -> Option<&'a Monitor> {
    monitors
        .iter()
        .find(|monitor| !saved.is_empty() && monitor.name == saved)
        .or_else(|| monitors.get(primary))
        .or_else(|| monitors.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, x: i32, w: u32, h: u32, scale: f64) -> Monitor {
        Monitor {
            name: name.into(),
            x,
            y: 0,
            w,
            h,
            scale,
        }
    }

    #[test]
    fn oversized_or_offscreen_rects_are_clamped_into_the_work_area() {
        let m = monitor("A", 0, 1280, 720, 1.0);
        let wild = OverlayRect {
            monitor: "A".into(),
            x: 5000.0,
            y: -50.0,
            w: 9000.0,
            h: 10.0,
            ..OverlayRect::default()
        };
        let p = to_physical(&wild, &m);
        assert!(p.x >= 0 && p.x as u32 + p.w <= 1280);
        assert!(p.y >= 0 && p.y as u32 + p.h <= 720);
        assert!(f64::from(p.h) >= MIN_H);
    }
}
