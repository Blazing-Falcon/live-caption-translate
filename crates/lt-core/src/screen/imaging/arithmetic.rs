use super::parallel_rows;
use super::{saturate, ImageBuffer};

#[derive(Clone, Copy)]
pub enum Operation {
    Add,
    Subtract,
    ReverseSubtract,
    Multiply,
    Divide,
    Min,
    Max,
    And,
    Or,
    AbsoluteDifference,
}
fn apply(a: f32, b: f32, operation: Operation, bytes: bool) -> f32 {
    match operation {
        Operation::Add => a + b,
        Operation::Subtract => {
            if bytes {
                (a - b).max(0.0)
            } else {
                a - b
            }
        }
        Operation::ReverseSubtract => b - a,
        Operation::Multiply => a * b,
        Operation::Divide => a / b,
        Operation::Min => {
            if a <= b {
                a
            } else {
                b
            }
        }
        Operation::Max => {
            if a >= b {
                a
            } else {
                b
            }
        }
        Operation::And => f32::from(a as u8 & b as u8),
        Operation::Or => f32::from(a as u8 | b as u8),
        Operation::AbsoluteDifference => (a - b).abs(),
    }
}
pub fn binary(a: &ImageBuffer, b: &ImageBuffer, target: &ImageBuffer, operation: Operation) {
    a.same_shape(b);
    a.same_shape(target);
    parallel_rows::for_each(a.height, a.width * a.channels(), |y| {
        for x in 0..a.width * a.channels() {
            write(
                target,
                y,
                x,
                apply(
                    a.value(y, x),
                    b.value(y, x),
                    operation,
                    !a.pixel_type.is_float(),
                ),
            );
        }
    });
}
pub fn scalar(a: &ImageBuffer, value: f64, target: &ImageBuffer, operation: Operation) {
    a.same_shape(target);
    let value = if a.pixel_type.is_float() {
        value as f32
    } else {
        f32::from(saturate::to_byte(value))
    };
    parallel_rows::for_each(a.height, a.width * a.channels(), |y| {
        for x in 0..a.width * a.channels() {
            write(
                target,
                y,
                x,
                apply(a.value(y, x), value, operation, !a.pixel_type.is_float()),
            );
        }
    });
}
pub fn divide(a: &ImageBuffer, b: &ImageBuffer, target: &ImageBuffer, scale: f64) {
    let s = scale as f32;
    if s == 1.0 {
        binary(a, b, target, Operation::Divide)
    } else {
        let scaled = ImageBuffer::new(a.width, a.height, a.pixel_type);
        scalar(a, f64::from(s), &scaled, Operation::Multiply);
        binary(&scaled, b, target, Operation::Divide)
    }
}
pub fn not(a: &ImageBuffer, target: &ImageBuffer) {
    a.same_shape(target);
    assert!(!a.pixel_type.is_float());
    for y in 0..a.height {
        for x in 0..a.row_bytes() {
            target.set_byte(y, x, !a.byte(y, x));
        }
    }
}
pub fn absolute(a: &ImageBuffer, target: &ImageBuffer) {
    a.same_shape(target);
    assert!(a.pixel_type.is_float());
    for y in 0..a.height {
        for x in 0..a.width * a.channels() {
            target.set_float(y, x, a.float(y, x).abs());
        }
    }
}
pub fn add_to_bytes(bytes: &ImageBuffer, floats: &ImageBuffer, target: &ImageBuffer) {
    bytes.same_shape(target);
    assert_eq!(
        (bytes.width, bytes.height, bytes.channels()),
        (floats.width, floats.height, floats.channels())
    );
    for y in 0..bytes.height {
        for x in 0..bytes.width * bytes.channels() {
            target.set_byte(
                y,
                x,
                (i32::from(bytes.byte(y, x)) + saturate::to_int(f64::from(floats.float(y, x))))
                    .clamp(0, 255) as u8,
            );
        }
    }
}
pub fn add_weighted(
    a: &ImageBuffer,
    alpha: f64,
    b: &ImageBuffer,
    beta: f64,
    gamma: f64,
    target: &ImageBuffer,
) {
    a.same_shape(b);
    a.same_shape(target);
    for y in 0..a.height {
        for x in 0..a.row_bytes() {
            let v = f32::from(a.byte(y, x)).mul_add(
                alpha as f32,
                f32::from(b.byte(y, x)).mul_add(beta as f32, gamma as f32),
            );
            target.set_byte(y, x, saturate::to_byte(f64::from(v)));
        }
    }
}

fn write(target: &ImageBuffer, y: usize, x: usize, value: f32) {
    if target.pixel_type.is_float() {
        target.set_float(y, x, value);
    } else {
        target.set_byte(y, x, value as u8);
    }
}
