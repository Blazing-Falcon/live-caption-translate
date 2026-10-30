use super::parallel_rows;
use super::{saturate, ImageBuffer};
const COEFFICIENT_BITS: i32 = 11;
const COEFFICIENT_SCALE: i32 = 1 << COEFFICIENT_BITS;
const DOUBLE_EPSILON: f64 = 2.220446049250313e-16;
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Interpolation {
    Area,
    Linear,
    Nearest,
}
pub fn to(
    source: &ImageBuffer,
    width: usize,
    height: usize,
    interpolation: Interpolation,
) -> ImageBuffer {
    let target = ImageBuffer::new(width, height, source.pixel_type);
    to_buffer(source, &target, interpolation);
    target
}
pub fn to_buffer(source: &ImageBuffer, target: &ImageBuffer, mut interpolation: Interpolation) {
    assert_eq!(source.pixel_type, target.pixel_type);
    assert!(!source.is_empty() && !target.is_empty());
    if (source.width, source.height) == (target.width, target.height) {
        source.copy_to(target);
        return;
    }
    let inverse_x = target.width as f64 / source.width as f64;
    let inverse_y = target.height as f64 / source.height as f64;
    let scale_x = 1.0 / inverse_x;
    let scale_y = 1.0 / inverse_y;
    if interpolation == Interpolation::Nearest {
        for y in 0..target.height {
            let sy = ((y as f64 * scale_y).floor() as usize).min(source.height - 1);
            for x in 0..target.width {
                let sx = ((x as f64 * scale_x).floor() as usize).min(source.width - 1);
                for c in 0..source.channels() {
                    if source.pixel_type.is_float() {
                        target.set_float(
                            y,
                            x * source.channels() + c,
                            source.float(sy, sx * source.channels() + c),
                        );
                    } else {
                        target.set_byte(
                            y,
                            x * source.channels() + c,
                            source.byte(sy, sx * source.channels() + c),
                        );
                    }
                }
            }
        }
        return;
    }
    let integer_x = saturate::to_int(scale_x);
    let integer_y = saturate::to_int(scale_y);
    let exact = (scale_x - f64::from(integer_x)).abs() < DOUBLE_EPSILON
        && (scale_y - f64::from(integer_y)).abs() < DOUBLE_EPSILON;
    if interpolation == Interpolation::Linear && exact && integer_x == 2 && integer_y == 2 {
        interpolation = Interpolation::Area;
    }
    if interpolation == Interpolation::Area {
        assert!(scale_x >= 1.0 && scale_y >= 1.0);
        if exact {
            area_exact(source, target, integer_x as usize, integer_y as usize);
        } else {
            area(source, target, scale_x, scale_y);
        }
        return;
    }
    if source.pixel_type.is_float() {
        linear_float(source, target, scale_x, scale_y);
    } else {
        linear_byte(source, target, scale_x, scale_y);
    }
}
fn area_exact(source: &ImageBuffer, target: &ImageBuffer, sx: usize, sy: usize) {
    let cn = source.channels();
    let scale = 1.0f32 / (sx * sy) as f32;
    parallel_rows::for_each(target.height, target.width * cn * sx * sy, |y| {
        for x in 0..target.width * cn {
            let i = (x / cn) * sx * cn + x % cn;
            if !source.pixel_type.is_float() {
                let mut sum = 0i32;
                for r in 0..sy {
                    for c in 0..sx {
                        sum += i32::from(source.byte(y * sy + r, i + c * cn));
                    }
                }
                let v = if sx == 2 && sy == 2 {
                    ((sum + 2) >> 2) as u8
                } else {
                    saturate::to_byte(f64::from(sum as f32 * scale))
                };
                target.set_byte(y, x, v);
            } else if sx == 2 && sy == 2 && cn == 1 {
                let a = source.float(y * 2, 2 * x);
                let b = source.float(y * 2, 2 * x + 1);
                let c = source.float(y * 2 + 1, 2 * x);
                let d = source.float(y * 2 + 1, 2 * x + 1);
                target.set_float(y, x, ((a + b) + (c + d)) * 0.25);
            } else {
                let mut v = Vec::with_capacity(sx * sy);
                for r in 0..sy {
                    for c in 0..sx {
                        v.push(source.float(y * sy + r, i + c * cn));
                    }
                }
                let mut sum = 0f32;
                let mut k = 0;
                while k + 4 <= v.len() {
                    sum += v[k] + v[k + 1] + v[k + 2] + v[k + 3];
                    k += 4;
                }
                while k < v.len() {
                    sum += v[k];
                    k += 1;
                }
                target.set_float(y, x, sum * scale);
            }
        }
    });
}
fn area_table(source_size: usize, target_size: usize, scale: f64) -> Vec<Vec<(usize, f32)>> {
    let mut table = vec![Vec::new(); target_size];
    for (dx, entries) in table.iter_mut().enumerate() {
        let fsx1 = dx as f64 * scale;
        let fsx2 = fsx1 + scale;
        let cell_width = scale.min(source_size as f64 - fsx1);
        let mut sx1 = fsx1.ceil() as usize;
        let sx2 = (fsx2.floor() as usize).min(source_size - 1);
        sx1 = sx1.min(sx2);
        if sx1 as f64 - fsx1 > 1e-3 {
            entries.push((sx1 - 1, ((sx1 as f64 - fsx1) / cell_width) as f32));
        }
        for sx in sx1..sx2 {
            entries.push((sx, (1.0 / cell_width) as f32));
        }
        if fsx2 - sx2 as f64 > 1e-3 {
            entries.push((
                sx2,
                ((fsx2 - sx2 as f64).min(1.0).min(cell_width) / cell_width) as f32,
            ));
        }
    }
    table
}
fn area(source: &ImageBuffer, target: &ImageBuffer, scale_x: f64, scale_y: f64) {
    let xt = area_table(source.width, target.width, scale_x);
    let yt = area_table(source.height, target.height, scale_y);
    let cn = source.channels();
    parallel_rows::for_each(
        target.height,
        ((target.width * cn) as f64 * scale_x * scale_y) as usize,
        |dy| {
            let rows = &yt[dy];
            let mut sum = vec![0f32; target.width * cn];
            for (sy, beta) in rows {
                for (px, cols) in xt.iter().enumerate() {
                    for c in 0..cn {
                        let (mut t, mut first) = (0.0f32, true);
                        for (sx, alpha) in cols {
                            let v = source.value(*sy, sx * cn + c) * alpha;
                            if first {
                                t = v;
                                first = false;
                            } else {
                                t += v;
                            }
                        }
                        sum[px * cn + c] += beta * t;
                    }
                }
            }
            for (i, v) in sum.iter().enumerate() {
                target.set_value(dy, i, *v);
            }
        },
    );
}
fn tables(
    source_size: usize,
    target_size: usize,
    scale: f64,
    precise: bool,
) -> (Vec<usize>, Vec<[f32; 2]>, usize) {
    let mut offsets = vec![0; target_size];
    let mut weights = vec![[0.0; 2]; target_size];
    let mut limit = target_size;
    for dx in 0..target_size {
        let (mut sx, mut fx) = if precise {
            let p = (dx as f64 + 0.5) * scale - 0.5;
            let sx = p.floor() as i32;
            (sx, (p - f64::from(sx)) as f32)
        } else {
            let fx = ((dx as f64 + 0.5) * scale - 0.5) as f32;
            let sx = fx.floor() as i32;
            (sx, fx - sx as f32)
        };
        if sx < 0 {
            fx = 0.0;
            sx = 0;
        }
        if sx + 1 >= source_size as i32 {
            limit = limit.min(dx);
            if sx >= source_size as i32 - 1 {
                fx = 0.0;
                sx = source_size as i32 - 1;
            }
        }
        offsets[dx] = sx as usize;
        weights[dx] = [1.0 - fx, fx];
    }
    (offsets, weights, limit)
}
fn linear_byte(source: &ImageBuffer, target: &ImageBuffer, scale_x: f64, scale_y: f64) {
    let cn = source.channels();
    let (xofs, xa, xmax) = tables(source.width, target.width, scale_x, false);
    parallel_rows::blocks(target.height, target.width * cn * 3, |from, to| {
        for dy in from..to {
            let mut fy = ((dy as f64 + 0.5) * scale_y - 0.5) as f32;
            let sy = fy.floor() as i32;
            fy -= sy as f32;
            let b0 = i32::from(saturate::to_short((1.0 - fy) * COEFFICIENT_SCALE as f32));
            let b1 = i32::from(saturate::to_short(fy * COEFFICIENT_SCALE as f32));
            let y0 = sy.clamp(0, source.height as i32 - 1) as usize;
            let y1 = (sy + 1).clamp(0, source.height as i32 - 1) as usize;
            for dx in 0..target.width {
                let sx = xofs[dx];
                let (a0, a1, sx1) = if dx < xmax {
                    (
                        i32::from(saturate::to_short(xa[dx][0] * COEFFICIENT_SCALE as f32)),
                        i32::from(saturate::to_short(xa[dx][1] * COEFFICIENT_SCALE as f32)),
                        sx + 1,
                    )
                } else {
                    (COEFFICIENT_SCALE, 0, sx)
                };
                for c in 0..cn {
                    let r0 = i32::from(source.byte(y0, sx * cn + c)) * a0
                        + i32::from(source.byte(y0, sx1 * cn + c)) * a1;
                    let r1 = i32::from(source.byte(y1, sx * cn + c)) * a0
                        + i32::from(source.byte(y1, sx1 * cn + c)) * a1;
                    target.set_byte(
                        dy,
                        dx * cn + c,
                        ((((b0 * (r0 >> 4)) >> 16) + ((b1 * (r1 >> 4)) >> 16) + 2) >> 2) as u8,
                    );
                }
            }
        }
    });
}
fn linear_float(source: &ImageBuffer, target: &ImageBuffer, scale_x: f64, scale_y: f64) {
    let cn = source.channels();
    let (xofs, xa, xmax) = tables(source.width, target.width, scale_x, true);
    parallel_rows::for_each(target.height, target.width * cn * 4, |dy| {
        let position = (dy as f64 + 0.5) * scale_y - 0.5;
        let sy = position.floor() as i32;
        let fy = (position - f64::from(sy)) as f32;
        let y0 = sy.clamp(0, source.height as i32 - 1) as usize;
        let y1 = (sy + 1).clamp(0, source.height as i32 - 1) as usize;
        for dx in 0..target.width {
            let sx = xofs[dx];
            for c in 0..cn {
                let a = source.float(y0, sx * cn + c);
                let b = source.float(y1, sx * cn + c);
                let (r0, r1) = if dx < xmax {
                    (
                        (source.float(y0, (sx + 1) * cn + c) - a).mul_add(xa[dx][1], a),
                        (source.float(y1, (sx + 1) * cn + c) - b).mul_add(xa[dx][1], b),
                    )
                } else {
                    (a, b)
                };
                target.set_float(dy, dx * cn + c, (r1 - r0).mul_add(fy, r0));
            }
        }
    });
}
