use super::parallel_rows;
use super::{saturate, ImageBuffer, PixelType};

#[derive(Clone, Copy)]
pub enum Border {
    Constant,
    Replicate,
    Reflect101,
}
fn interpolate(mut p: i32, length: usize, border: Border) -> Option<usize> {
    let length = length as i32;
    if p >= 0 && p < length {
        return Some(p as usize);
    }
    match border {
        Border::Constant => return None,
        Border::Replicate => return Some(if p < 0 { 0 } else { length as usize - 1 }),
        Border::Reflect101 => {
            if length == 1 {
                return Some(0);
            }
            while p < 0 || p >= length {
                if p < 0 {
                    p = -p;
                } else {
                    p = length - 1 - (p - length) - 1;
                }
            }
        }
    }
    Some(p as usize)
}
pub fn neighbourhood(source: &ImageBuffer, rx: usize, ry: usize, border: Border) -> ImageBuffer {
    let padded = ImageBuffer::new(
        source.width + rx * 2,
        source.height + ry * 2,
        source.pixel_type,
    );
    for y in 0..padded.height {
        let Some(sy) = interpolate(
            source.origin_y as i32 + y as i32 - ry as i32,
            source.root_height,
            border,
        ) else {
            continue;
        };
        for x in 0..padded.width {
            let Some(sx) = interpolate(
                source.origin_x as i32 + x as i32 - rx as i32,
                source.root_width,
                border,
            ) else {
                continue;
            };
            for c in 0..source.channels() {
                if source.pixel_type.is_float() {
                    padded.set_float(y, x * source.channels() + c, source.root_value(sx, sy, c));
                } else {
                    padded.set_byte(y, x * source.channels() + c, source.root_byte(sx, sy, c));
                }
            }
        }
    }
    padded
}
fn row_sums(padded: &ImageBuffer, width: usize, size: usize) -> Vec<i32> {
    let mut sums = vec![0; width * padded.height];
    parallel_rows::slice_rows(&mut sums, padded.height, width, padded.width, |y, row| {
        let mut s = 0;
        for dx in 0..size {
            s += i32::from(padded.byte(y, dx));
        }
        row[0] = s;
        for (x, value) in row.iter_mut().enumerate().skip(1) {
            s += i32::from(padded.byte(y, x + size - 1)) - i32::from(padded.byte(y, x - 1));
            *value = s;
        }
    });
    sums
}
fn column_sums(
    sums: &[i32],
    width: usize,
    height: usize,
    size: usize,
    target: &ImageBuffer,
    div_scale: u32,
    div_delta: u32,
) {
    parallel_rows::blocks(width, height + size, |from, to| {
        let mut running = vec![0; to - from];
        for dy in 0..size - 1 {
            for (i, value) in running.iter_mut().enumerate() {
                *value += sums[dy * width + from + i];
            }
        }
        for y in 0..height {
            for (i, value) in running.iter_mut().enumerate() {
                let x = from + i;
                let s = *value + sums[(y + size - 1) * width + x];
                if target.pixel_type == PixelType::U8C1 {
                    target.set_byte(
                        y,
                        x,
                        ((s as u32 + div_delta).wrapping_mul(div_scale) >> 23) as u8,
                    );
                } else {
                    target.set_float(y, x, s as f32);
                }
                *value = s - sums[y * width + x];
            }
        }
    });
}

pub fn blur(source: &ImageBuffer, target: &ImageBuffer, size: usize) {
    assert_eq!(source.channels(), 1);
    source.same_shape(target);
    assert!(size % 2 == 1);
    if source.is_empty() {
        return;
    }
    let (width, height) = (source.width, source.height);
    let padded = neighbourhood(source, size / 2, size / 2, Border::Reflect101);
    if source.pixel_type == PixelType::U8C1 {
        assert!(size * size <= 256);
        let divisor = size * size;
        let exact = (1 << 23) as f64 / divisor as f64;
        let mut div_scale = exact.floor() as u32;
        let mut div_delta = (divisor / 2) as u32;
        if exact - f64::from(div_scale) < 0.5 {
            div_delta += 1;
        } else {
            div_scale += 1;
        }
        let sums = row_sums(&padded, width, size);
        column_sums(&sums, width, height, size, target, div_scale, div_delta);
        return;
    }
    let scale = 1.0 / (size * size) as f64;
    let rows = padded.height;
    let mut sums = vec![0f64; rows * width];
    parallel_rows::slice_rows(&mut sums, rows, width, width * size, |py, row| {
        for (x, value) in row.iter_mut().enumerate() {
            let mut v = f64::from(padded.float(py, x));
            for dx in 1..size {
                v += f64::from(padded.float(py, x + dx));
            }
            *value = v;
        }
    });
    parallel_rows::blocks(width, rows, |from, to| {
        let mut running = vec![0f64; to - from];
        for py in 0..rows {
            if py < size - 1 {
                for (i, value) in running.iter_mut().enumerate() {
                    *value += sums[py * width + from + i];
                }
                continue;
            }
            for (i, value) in running.iter_mut().enumerate() {
                let x = from + i;
                let s0 = *value + sums[py * width + x];
                target.set_float(py - (size - 1), x, (s0 * scale) as f32);
                *value = s0 - sums[(py - (size - 1)) * width + x];
            }
        }
    });
}

pub fn box_sum(source: &ImageBuffer, size: usize) -> ImageBuffer {
    assert_eq!(source.pixel_type, PixelType::U8C1);
    assert!(size % 2 == 1);
    let target = ImageBuffer::new(source.width, source.height, PixelType::F32C1);
    if source.is_empty() {
        return target;
    }
    let padded = neighbourhood(source, size / 2, size / 2, Border::Constant);
    let sums = row_sums(&padded, source.width, size);
    column_sums(&sums, source.width, source.height, size, &target, 0, 0);
    target
}
pub fn laplacian(source: &ImageBuffer) -> ImageBuffer {
    laplacian_with_form(source, 0)
}
pub fn laplacian_with_form(source: &ImageBuffer, form: i32) -> ImageBuffer {
    assert_eq!(source.pixel_type, PixelType::F32C1);
    let target = ImageBuffer::new(source.width, source.height, PixelType::F32C1);
    if source.is_empty() {
        return target;
    }
    let p = neighbourhood(source, 1, 1, Border::Reflect101);
    parallel_rows::for_each(source.height, source.width * 5, |y| {
        for x in 0..source.width {
            target.set_float(
                y,
                x,
                laplacian_at(
                    p.float(y, x),
                    p.float(y, x + 2),
                    p.float(y + 1, x + 1),
                    p.float(y + 2, x),
                    p.float(y + 2, x + 2),
                    form,
                ),
            );
        }
    });
    target
}
fn laplacian_at(tl: f32, tr: f32, centre: f32, bl: f32, br: f32, form: i32) -> f32 {
    match form {
        0 => 2.0 * tl + 2.0 * tr + -8.0 * centre + 2.0 * bl + 2.0 * br,
        1 => 2.0f32.mul_add(
            br,
            2.0f32.mul_add(bl, (-8.0f32).mul_add(centre, 2.0f32.mul_add(tr, 2.0 * tl))),
        ),
        2 => (tl + tr + bl + br) * 2.0 - centre * 8.0,
        3 => ((tl + tr) + (bl + br)) * 2.0 - 8.0 * centre,
        4 => 2.0 * (tl + tr + bl + br - 4.0 * centre),
        5 => (tl + tr + bl + br - 4.0 * centre) * 2.0,
        6 => tl * 2.0 + tr * 2.0 + bl * 2.0 + br * 2.0 - centre * 8.0,
        7 => (-8.0f32).mul_add(centre, (tl + tr + bl + br) * 2.0),
        _ => 2.0 * tl + 2.0 * tr + 2.0 * bl + 2.0 * br - 8.0 * centre,
    }
}
fn gaussian_kernel(size: usize, sigma: f64) -> Vec<i32> {
    if size == 1 {
        return vec![256];
    }
    let fixed: Option<Vec<f64>> = match size {
        3 => Some(vec![0.25, 0.5, 0.25]),
        5 => Some(vec![0.0625, 0.25, 0.375, 0.25, 0.0625]),
        7 => Some(vec![
            0.03125, 0.109375, 0.21875, 0.28125, 0.21875, 0.109375, 0.03125,
        ]),
        9 => Some(
            [4, 13, 30, 51, 60, 51, 30, 13, 4]
                .iter()
                .map(|v| f64::from(*v) / 256.0)
                .collect(),
        ),
        _ => None,
    };
    let taps = match fixed {
        Some(taps) if sigma <= 0.0 => taps,
        _ => {
            let sigma_x = if sigma > 0.0 {
                sigma
            } else {
                size as f64 * 0.15 + 0.35
            };
            let scale = -0.125 / (sigma_x * sigma_x);
            let half = (size - 1) / 2;
            let mut values = vec![0.0; half];
            let mut sum = 0.0;
            for (i, v) in values.iter_mut().enumerate() {
                let x = 1 - size as i32 + 2 * i as i32;
                *v = (f64::from(x * x) * scale).exp();
                sum += *v;
            }
            sum *= 2.0;
            sum += 1.0;
            let inverse = 1.0 / sum;
            let mut taps = vec![0.0; size];
            for i in 0..half {
                let t = values[i] * inverse;
                taps[i] = t;
                taps[size - 1 - i] = t;
            }
            taps[half] = inverse;
            taps
        }
    };
    let mut result = vec![0; size];
    let mut error = 0.0;
    let mut total = 0i64;
    for i in 0..size / 2 {
        let adjusted = taps[i] * 256.0 + error;
        let v = adjusted.round_ties_even() as i64;
        error = adjusted - v as f64;
        result[i] = v as i32;
        result[size - 1 - i] = v as i32;
        total += v;
    }
    result[size / 2] = (256 - total * 2) as i32;
    result
}
pub fn gaussian_blur(source: &ImageBuffer, sigma: f64) -> ImageBuffer {
    assert!(!source.pixel_type.is_float());
    let size = (saturate::to_int(sigma * 3.0 * 2.0 + 1.0) | 1) as usize;
    let target = ImageBuffer::new(source.width, source.height, source.pixel_type);
    if source.is_empty() {
        return target;
    }
    let (width, height, channels) = (source.width, source.height, source.channels());
    if size == 1 || (width == 1 && height == 1) {
        source.copy_to(&target);
        return target;
    }
    let size_x = if width == 1 { 1 } else { size };
    let size_y = if height == 1 { 1 } else { size };
    let kernel_x = gaussian_kernel(size_x, sigma);
    let kernel_y = gaussian_kernel(size_y, sigma);
    let padded = neighbourhood(source, size_x / 2, size_y / 2, Border::Replicate);
    let n = width * channels;
    let mut across = vec![0i32; padded.height * n];
    parallel_rows::slice_rows(&mut across, padded.height, n, n * size_x, |py, row| {
        for (i, value) in row.iter_mut().enumerate() {
            let mut s = 0;
            for (t, k) in kernel_x.iter().enumerate() {
                s += i32::from(padded.byte(py, i + t * channels)) * k;
            }
            *value = s;
        }
    });
    parallel_rows::for_each(height, n * size_y, |y| {
        for i in 0..n {
            let mut s = 0i64;
            for (t, k) in kernel_y.iter().enumerate() {
                s += i64::from(across[(y + t) * n + i]) * i64::from(*k);
            }
            target.set_byte(y, i, 255.min((s + (1 << 15)) >> 16) as u8);
        }
    });
    target
}
