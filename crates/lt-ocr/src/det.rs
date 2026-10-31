//! Detector frame preparation and source coordinate mapping.

use lt_core::screen::{
    detection::{self, Detection},
    geom::PointF,
    raster::Raster,
    repair,
};
use lt_core::screen::{ImageView, Rect};

/// Longest side the detector sees; larger frames are scaled down to this.
const MAX_SIDE: u32 = 2048;
/// White outer padding keeps edge text inside the detector frame.
const BORDER: u32 = 8;
/// The detector input sides are rounded up to a multiple of this, the stride of its network.
const ALIGN: u32 = 32;

/// How the source image maps onto the detector input.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    pub width: u32,
    pub height: u32,
    pub resized_w: u32,
    pub resized_h: u32,
    pub input_w: u32,
    pub input_h: u32,
}

impl Geometry {
    /// Downscales only, keeping the aspect ratio; pads instead of stretching to reach 32.
    pub fn new(width: u32, height: u32) -> Self {
        Self::with_limit(width, height, MAX_SIDE)
    }

    fn with_limit(width: u32, height: u32, limit: u32) -> Self {
        let scale = if limit == 0 {
            1.0
        } else {
            (limit as f64 / width.max(height) as f64).min(1.0)
        };
        let resized = |side: u32| ((side as f64 * scale).round_ties_even() as u32).max(1);
        let (resized_w, resized_h) = (resized(width), resized(height));
        let input = |side: u32| (side + 2 * BORDER).div_ceil(ALIGN) * ALIGN;
        Self {
            width,
            height,
            resized_w,
            resized_h,
            input_w: input(resized_w),
            input_h: input(resized_h),
        }
    }

    pub fn source_points(&self, points: &[(i32, i32); 4]) -> [PointF; 4] {
        let ratio_x = f64::from(self.resized_w) / f64::from(self.width);
        let ratio_y = f64::from(self.resized_h) / f64::from(self.height);
        points.map(|(x, y)| PointF {
            x: (f64::from((x - BORDER as i32).clamp(0, (self.input_w - 2 * BORDER) as i32))
                / ratio_x)
                .round_ties_even(),
            y: (f64::from((y - BORDER as i32).clamp(0, (self.input_h - 2 * BORDER) as i32))
                / ratio_y)
                .round_ties_even(),
        })
    }

    /// Maps integer detector corners through the padding and each axis's scale.
    pub fn source_rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Rect> {
        let scale_x = self.resized_w as f64 / self.width as f64;
        let scale_y = self.resized_h as f64 / self.height as f64;
        let map = |value: f32, scale: f64, limit: u32| {
            (f64::from((value as i32 - BORDER as i32).clamp(0, limit as i32)) / scale)
                .round_ties_even() as u32
        };
        let left = map(x0, scale_x, self.input_w - 2 * BORDER);
        let top = map(y0, scale_y, self.input_h - 2 * BORDER);
        let right = map(x1, scale_x, self.input_w - 2 * BORDER);
        let bottom = map(y1, scale_y, self.input_h - 2 * BORDER);
        (right > left && bottom > top).then(|| Rect {
            x: left as i32,
            y: top as i32,
            w: right - left,
            h: bottom - top,
        })
    }
}

pub(crate) fn frame(image: &ImageView<'_>, geometry: &Geometry) -> Raster {
    let scaled = (geometry.resized_w != image.width || geometry.resized_h != image.height)
        .then(|| Raster::resize(image, geometry.resized_w, geometry.resized_h));
    let body = scaled.as_ref().map_or(*image, Raster::view);
    let (width, height) = (geometry.input_w - 2 * BORDER, geometry.input_h - 2 * BORDER);
    let mut aligned = Raster {
        width,
        height,
        bgra: vec![0; width as usize * height as usize * 4],
    };
    for y in 0..body.height {
        aligned.bgra[(y * width * 4) as usize..][..body.width as usize * 4]
            .copy_from_slice(body.row(y));
    }
    let filtered = Raster::resample(&aligned.view(), width, height);
    let mut padded = Raster {
        width: geometry.input_w,
        height: geometry.input_h,
        bgra: vec![255; geometry.input_w as usize * geometry.input_h as usize * 4],
    };
    for y in 0..height {
        for x in 0..width {
            let src = ((y * width + x) * 4) as usize;
            let dst = (((y + BORDER) * padded.width + x + BORDER) * 4) as usize;
            let alpha = filtered.bgra[src + 3];
            for channel in 0..3 {
                padded.bgra[dst + channel] =
                    filtered.bgra[src + channel].saturating_add(255 - alpha);
            }
        }
    }
    padded
}

pub(crate) fn input(frame: &Raster) -> Vec<f32> {
    let resized = Raster::resize(&frame.view(), frame.width, frame.height);
    let plane = frame.width as usize * frame.height as usize;
    let mut tensor = vec![0.0; 3 * plane];
    /// Maps bytes to the range the detector was trained on, centred on zero.
    const NORM: f32 = 1.0 / 127.5;
    for (i, pixel) in resized.bgra.as_chunks::<4>().0.iter().enumerate() {
        for channel in 0..3 {
            tensor[channel * plane + i] = (f32::from(pixel[channel]) - 127.5) * NORM;
        }
    }
    tensor
}

pub(crate) fn boxes(map: &[f32], geometry: &Geometry, source: &ImageView<'_>) -> Vec<Detection> {
    let boxes = detection::boxes(map, geometry.input_w as usize, geometry.input_h as usize);
    let bounds: Vec<_> = boxes
        .iter()
        .map(|b| {
            let points = geometry.source_points(&b.points);
            repair::Bounds {
                left: points
                    .iter()
                    .map(|p| p.x as f32)
                    .fold(f32::INFINITY, f32::min),
                top: points
                    .iter()
                    .map(|p| p.y as f32)
                    .fold(f32::INFINITY, f32::min),
                right: points
                    .iter()
                    .map(|p| p.x as f32)
                    .fold(f32::NEG_INFINITY, f32::max),
                bottom: points
                    .iter()
                    .map(|p| p.y as f32)
                    .fold(f32::NEG_INFINITY, f32::max),
            }
        })
        .collect();
    let repairs = repair::find(source, &bounds);
    if repairs.is_empty() {
        return boxes;
    }
    let mut result = Vec::new();
    for (i, b) in boxes.iter().enumerate() {
        let Some(repair) = repairs.iter().find(|r| r.owners.contains(&i)) else {
            result.push(b.clone());
            continue;
        };
        if Some(&i) != repair.owners.iter().min() {
            continue;
        }
        let ratio_x = f64::from(geometry.resized_w) / f64::from(geometry.width);
        let ratio_y = f64::from(geometry.resized_h) / f64::from(geometry.height);
        let bounds = repair.bounds;
        let left = (f64::from(bounds.left) * ratio_x + f64::from(BORDER)).floor() as i32;
        let right = (f64::from(bounds.right) * ratio_x + f64::from(BORDER)).ceil() as i32;
        let top = (f64::from(bounds.top) * ratio_y + f64::from(BORDER)).floor() as i32;
        let bottom = (f64::from(bounds.bottom) * ratio_y + f64::from(BORDER)).ceil() as i32;
        result.push(Detection {
            points: [(left, top), (right, top), (right, bottom), (left, bottom)],
            score: repair
                .owners
                .iter()
                .map(|&j| boxes[j].score)
                .fold(f32::INFINITY, f32::min),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    fn requested(width: u32, height: u32, fraction: f64) -> u32 {
        let native = width.max(height);
        if native < 800 {
            native
        } else {
            ((native as f64 * fraction) as u32).div_ceil(32).max(10) * 32
        }
    }

    #[test]
    fn detector_preserves_the_requested_aspect_ratio() {
        for (width, height) in [
            (465, 76),
            (450, 150),
            (1150, 240),
            (1825, 223),
            (1824, 129),
            (264, 56),
            (270, 60),
            (2473, 892),
            (3840, 2160),
        ] {
            let frame = Geometry::with_limit(width, height, requested(width, height, 0.85));
            let ratio_x = f64::from(frame.resized_w) / f64::from(width);
            let ratio_y = f64::from(frame.resized_h) / f64::from(height);
            assert!((0.995..=1.005).contains(&(ratio_x / ratio_y)));
        }
    }

    #[test]
    fn padded_detector_dimensions_need_no_stride_resize() {
        for (width, height) in [(465, 76), (1150, 240), (1825, 223), (2473, 892)] {
            let frame = Geometry::with_limit(width, height, requested(width, height, 0.85));
            assert_eq!(frame.input_w % 32, 0);
            assert_eq!(frame.input_h % 32, 0);
        }
    }

    #[test]
    fn detector_receives_the_requested_size_with_only_stride_extension() {
        for (width, height, fraction) in [(1150, 240, 0.85), (1825, 223, 0.85), (2473, 892, 0.68)] {
            let limit = requested(width, height, fraction);
            let frame = Geometry::with_limit(width, height, limit);
            let longest = frame.input_w.max(frame.input_h) - 2 * BORDER;
            assert!((limit..=limit + 31).contains(&longest));
        }
    }

    #[test]
    fn small_regions_are_not_resized() {
        let frame = Geometry::with_limit(465, 76, requested(465, 76, 0.85));
        assert_eq!(frame.resized_w, 465);
        assert_eq!(frame.resized_h, 76);
    }

    #[test]
    fn detector_boxes_map_back_through_the_border_and_the_downscale() {
        // Scale 1: 100 x 50 becomes 100 + 16 -> 128 by 50 + 16 -> 96; the extension is right and
        // bottom only, so only the border moves a box.
        let same = Geometry::new(100, 50);
        assert_eq!((same.input_w, same.input_h), (128, 96));
        assert_eq!(
            same.source_rect(18.0, 28.0, 58.0, 48.0),
            Some(rect(10, 20, 40, 20))
        );
        assert_eq!(
            same.source_rect(0.0, 0.0, 128.0, 96.0),
            Some(rect(0, 0, 112, 80))
        );
        assert_eq!(
            same.source_rect(110.0, 0.0, 128.0, 96.0),
            Some(rect(102, 0, 10, 80))
        );

        // 4096 x 1024 halves to 2048 x 512, padded to 2080 x 544.
        let half = Geometry::new(4096, 1024);
        assert_eq!((half.resized_w, half.resized_h), (2048, 512));
        assert_eq!((half.input_w, half.input_h), (2080, 544));
        assert_eq!(
            half.source_rect(18.0, 28.0, 58.0, 48.0),
            Some(rect(20, 40, 80, 40))
        );
        assert_eq!(
            half.source_rect(18.5, 8.0, 58.2, 9.0),
            Some(rect(20, 0, 80, 2))
        );

        // 3000 x 1000 has different rounding per axis: 2048 x 683.
        let odd = Geometry::new(3000, 1000);
        assert_eq!((odd.resized_w, odd.resized_h), (2048, 683));
        let back = odd.source_rect(8.0 + 1024.5, 8.0 + 341.0, 8.0 + 2048.0, 8.0 + 683.0);
        assert_eq!(back, Some(rect(1500, 499, 1500, 501)));
    }
}
