use super::parallel_rows;
use super::{
    arithmetic::{self, Operation},
    saturate, ImageBuffer, PixelType,
};
use std::sync::atomic::{AtomicU32, Ordering};

pub struct StructuringElement {
    pub width: usize,
    pub height: usize,
    pub half_widths: Vec<i32>,
    pub reach: usize,
}
impl StructuringElement {
    pub fn rectangle(width: usize, height: usize) -> Self {
        assert!(width > 0 && height > 0 && width % 2 == 1 && height % 2 == 1);
        Self {
            width,
            height,
            half_widths: vec![(width / 2) as i32; height],
            reach: width / 2,
        }
    }
    pub fn ellipse(width: usize, height: usize) -> Self {
        assert!(width > 0 && height > 0 && width % 2 == 1 && height % 2 == 1);
        let (r, c) = ((height / 2) as i32, (width / 2) as i32);
        let inverse = if r != 0 {
            1.0 / (f64::from(r) * f64::from(r))
        } else {
            0.0
        };
        let half_widths: Vec<_> = (0..height)
            .map(|i| {
                let dy = i as i32 - r;
                if dy.abs() > r {
                    return -1;
                }
                let dx =
                    saturate::to_int(f64::from(c) * (f64::from(r * r - dy * dy) * inverse).sqrt());
                let j1 = (c - dx).max(0);
                let j2 = (c + dx + 1).min(width as i32);
                if j2 > j1 {
                    c - j1
                } else {
                    -1
                }
            })
            .collect();
        let reach = *half_widths.iter().max().unwrap() as usize;
        Self {
            width,
            height,
            half_widths,
            reach,
        }
    }
}
pub fn dilate(
    source: &ImageBuffer,
    target: &ImageBuffer,
    element: &StructuringElement,
    iterations: usize,
) {
    run(source, target, element, iterations, true)
}
pub fn erode(
    source: &ImageBuffer,
    target: &ImageBuffer,
    element: &StructuringElement,
    iterations: usize,
) {
    run(source, target, element, iterations, false)
}
pub fn top_hat(source: &ImageBuffer, target: &ImageBuffer, element: &StructuringElement) {
    let opened = ImageBuffer::new(source.width, source.height, source.pixel_type);
    erode(source, &opened, element, 1);
    dilate(&opened, &opened, element, 1);
    arithmetic::binary(source, &opened, target, Operation::Subtract);
}
pub fn black_hat(source: &ImageBuffer, target: &ImageBuffer, element: &StructuringElement) {
    let closed = ImageBuffer::new(source.width, source.height, source.pixel_type);
    dilate(source, &closed, element, 1);
    erode(&closed, &closed, element, 1);
    arithmetic::binary(&closed, source, target, Operation::Subtract);
}
fn run(
    source: &ImageBuffer,
    target: &ImageBuffer,
    element: &StructuringElement,
    iterations: usize,
    maximum: bool,
) {
    assert!(matches!(
        source.pixel_type,
        PixelType::U8C1 | PixelType::F32C1
    ));
    source.same_shape(target);
    if source.is_empty() {
        return;
    }
    if iterations == 0 || element.width * element.height == 1 {
        source.copy_to(target);
        return;
    }
    let pass = ImageBuffer::new(source.width, source.height, source.pixel_type);
    one_pass(source, &pass, element, maximum);
    pass.copy_to(target);
    for _ in 1..iterations {
        one_pass(target, &pass, element, maximum);
        pass.copy_to(target);
    }
}
fn pick(a: f32, b: f32, maximum: bool) -> f32 {
    if maximum {
        a.max(b)
    } else {
        a.min(b)
    }
}
fn one_pass(
    source: &ImageBuffer,
    target: &ImageBuffer,
    element: &StructuringElement,
    maximum: bool,
) {
    let anchor_y = element.height / 2;
    let reach = element.reach;
    let left = source.origin_x.min(reach);
    let top = source.origin_y.min(anchor_y);
    let right = (source.root_width - source.origin_x - source.width).min(reach);
    let bottom =
        (source.root_height - source.origin_y - source.height).min(element.height - 1 - anchor_y);
    let (w, h) = (source.width + left + right, source.height + top + bottom);
    let size = w * h;
    let planes: Vec<_> = (0..size * (reach + 1)).map(|_| AtomicU32::new(0)).collect();
    let get = |i: usize| f32::from_bits(planes[i].load(Ordering::Relaxed));
    let set = |i: usize, v: f32| planes[i].store(v.to_bits(), Ordering::Relaxed);
    for y in 0..h {
        for x in 0..w {
            set(
                y * w + x,
                source.root_value(source.origin_x - left + x, source.origin_y - top + y, 0),
            );
        }
    }
    for a in 1..=reach {
        parallel_rows::for_each(h, w * 2, |y| {
            let prev = (a - 1) * size + y * w;
            let next = a * size + y * w;
            if w == 1 {
                set(next, get(prev));
                return;
            }
            set(next, pick(get(prev), get(prev + 1), maximum));
            for x in 1..w - 1 {
                let both = pick(get(prev + x - 1), get(prev + x + 1), maximum);
                set(next + x, pick(both, get(prev + x), maximum));
            }
            set(
                next + w - 1,
                pick(get(prev + w - 2), get(prev + w - 1), maximum),
            );
        });
    }
    parallel_rows::for_each(
        source.height,
        source.width * element.half_widths.len(),
        |oy| {
            let y = oy + top;
            let mut first = true;
            for (k, half) in element.half_widths.iter().enumerate() {
                if *half < 0 {
                    continue;
                }
                let sy = y as i32 + k as i32 - anchor_y as i32;
                if sy < 0 || sy >= h as i32 {
                    continue;
                }
                let offset = *half as usize * size + sy as usize * w + left;
                for x in 0..source.width {
                    let v = get(offset + x);
                    write(
                        target,
                        oy,
                        x,
                        if first {
                            v
                        } else {
                            pick(target.value(oy, x), v, maximum)
                        },
                    );
                }
                first = false;
            }
        },
    );
}

fn write(target: &ImageBuffer, y: usize, x: usize, value: f32) {
    if target.pixel_type.is_float() {
        target.set_float(y, x, value);
    } else {
        target.set_byte(y, x, value as u8);
    }
}
