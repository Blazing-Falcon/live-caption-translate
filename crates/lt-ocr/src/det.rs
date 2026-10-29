//! Text detection: a probability map from the detector, then one axis-aligned rect per region.

use crate::image::resize;
use lt_core::screen::{ImageView, Rect};

const MAX_SIDE: u32 = 2048;
/// Edge pixels repeated around the image, so text that touches the edge is still found whole.
const BORDER: u32 = 8;
const ALIGN: u32 = 32;
const BINARY: f32 = 0.2;
const BOX_SCORE: f32 = 0.4;
const UNCLIP: f32 = 1.4;
const MIN_SIDE: u32 = 3;

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
        let scale = (MAX_SIDE as f64 / width.max(height) as f64).min(1.0);
        let resized = |side: u32| ((side as f64 * scale).round() as u32).max(1);
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

    /// A box in detector-input pixels, edges `x0..x1` and `y0..y1`, in source pixels: rounded
    /// outward and clamped to the image. `None` when nothing of it is inside the image.
    pub fn source_rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Rect> {
        let scale_x = self.resized_w as f32 / self.width as f32;
        let scale_y = self.resized_h as f32 / self.height as f32;
        let map = |value: f32, scale: f32, limit: u32| {
            ((value - BORDER as f32) / scale).clamp(0.0, limit as f32)
        };
        let left = map(x0, scale_x, self.width).floor() as u32;
        let top = map(y0, scale_y, self.height).floor() as u32;
        let right = map(x1, scale_x, self.width).ceil() as u32;
        let bottom = map(y1, scale_y, self.height).ceil() as u32;
        (right > left && bottom > top).then(|| Rect {
            x: left as i32,
            y: top as i32,
            w: right - left,
            h: bottom - top,
        })
    }
}

/// The detector input: NCHW, BGR, each value `(v - 127.5) / 127.5`.
pub(crate) fn input(image: &ImageView<'_>, geometry: &Geometry) -> Vec<f32> {
    let full = Rect {
        x: 0,
        y: 0,
        w: image.width,
        h: image.height,
    };
    let resized = resize(image, full, geometry.resized_w, geometry.resized_h);
    let (w, h) = (geometry.input_w as usize, geometry.input_h as usize);
    let (rw, rh) = (geometry.resized_w as usize, geometry.resized_h as usize);
    let border = BORDER as usize;
    let mut data = vec![0.0_f32; 3 * w * h];
    for y in 0..h {
        let source_y = y.saturating_sub(border).min(rh - 1);
        for x in 0..w {
            let source_x = x.saturating_sub(border).min(rw - 1);
            let pixel = &resized[(source_y * rw + source_x) * 3..][..3];
            for (channel, value) in pixel.iter().enumerate() {
                data[channel * w * h + y * w + x] = (value - 127.5) / 127.5;
            }
        }
    }
    data
}

/// Rects of text in source pixels from the detector's probability map (`input_h` x `input_w`).
pub(crate) fn boxes(map: &[f32], geometry: &Geometry) -> Vec<Rect> {
    let (w, h) = (geometry.input_w as usize, geometry.input_h as usize);
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut rects = Vec::new();
    for start in 0..w * h {
        if seen[start] || map[start] <= BINARY {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let (mut sum, mut count) = (0.0_f64, 0_u32);
        while let Some(index) = stack.pop() {
            let (x, y) = (index % w, index / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            sum += f64::from(map[index]);
            count += 1;
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let next = ny * w + nx;
                    if !seen[next] && map[next] > BINARY {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
        if sum / f64::from(count) < f64::from(BOX_SCORE) || bw.min(bh) < MIN_SIDE as f32 {
            continue;
        }
        let grow = bw * bh * UNCLIP / (2.0 * (bw + bh));
        let rect = geometry.source_rect(
            x0 as f32 - grow,
            y0 as f32 - grow,
            (x1 + 1) as f32 + grow,
            (y1 + 1) as f32 + grow,
        );
        rects.extend(rect);
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
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
            Some(rect(0, 0, 100, 50))
        );
        assert_eq!(same.source_rect(110.0, 0.0, 128.0, 96.0), None);

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
            Some(rect(21, 0, 80, 2))
        );

        // 3000 x 1000 has different rounding per axis: 2048 x 683.
        let odd = Geometry::new(3000, 1000);
        assert_eq!((odd.resized_w, odd.resized_h), (2048, 683));
        let back = odd.source_rect(8.0 + 1024.5, 8.0 + 341.0, 8.0 + 2048.0, 8.0 + 683.0);
        assert_eq!(back, Some(rect(1500, 499, 1500, 501)));
    }
}
