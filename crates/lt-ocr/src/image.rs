//! Crop and resize for the two models. Pixels come out as BGR `f32` from 0 to 255, interleaved.

use lt_core::screen::{ImageView, Rect};

/// Resizes the part of `image` under `source` (inside the image) to `width` x `height`: area
/// averaging along an axis that shrinks, linear interpolation along one that grows.
pub(crate) fn resize(image: &ImageView<'_>, source: Rect, width: u32, height: u32) -> Vec<f32> {
    let columns = taps(source.w, width);
    let rows = taps(source.h, height);
    let (left, top) = (source.x as usize, source.y as usize);
    let width = width as usize;
    let mut horizontal = vec![0.0_f32; source.h as usize * width * 3];
    for (y, out) in horizontal.chunks_exact_mut(width * 3).enumerate() {
        let row = &image.row((top + y) as u32)[left * 4..];
        for (pixel, column) in out.as_chunks_mut::<3>().0.iter_mut().zip(&columns) {
            for &(x, weight) in column {
                let bgr = &row[x * 4..x * 4 + 3];
                for channel in 0..3 {
                    pixel[channel] += weight * f32::from(bgr[channel]);
                }
            }
        }
    }
    let mut out = vec![0.0_f32; height as usize * width * 3];
    for (line, row) in out.chunks_exact_mut(width * 3).zip(&rows) {
        for &(y, weight) in row {
            let from = &horizontal[y * width * 3..(y + 1) * width * 3];
            for (value, source) in line.iter_mut().zip(from) {
                *value += weight * source;
            }
        }
    }
    out
}

/// For each output index, the source indexes it reads and their weights.
fn taps(from: u32, to: u32) -> Vec<Vec<(usize, f32)>> {
    let scale = from as f64 / to as f64;
    let last = from as usize - 1;
    (0..to as usize)
        .map(|i| {
            if from == to {
                vec![(i, 1.0)]
            } else if scale > 1.0 {
                let (start, end) = (i as f64 * scale, (i + 1) as f64 * scale);
                let mut taps = Vec::with_capacity(scale.ceil() as usize + 1);
                let mut j = start.floor();
                while j < end {
                    let overlap = (j + 1.0).min(end) - j.max(start);
                    if overlap > 1e-9 {
                        taps.push(((j as usize).min(last), (overlap / scale) as f32));
                    }
                    j += 1.0;
                }
                taps
            } else {
                let center = ((i as f64 + 0.5) * scale - 0.5).clamp(0.0, last as f64);
                let j = center.floor() as usize;
                let fraction = (center - j as f64) as f32;
                vec![(j, 1.0 - fraction), ((j + 1).min(last), fraction)]
            }
        })
        .collect()
}
