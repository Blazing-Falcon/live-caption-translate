//! Channel merging, tiling, edge-replicating padding and cropping.

use super::parallel_rows;
use super::{ImageBuffer, PixelType};
pub fn merge(first: &ImageBuffer, second: &ImageBuffer, third: &ImageBuffer) -> ImageBuffer {
    assert_eq!(first.pixel_type, PixelType::F32C1);
    first.same_shape(second);
    first.same_shape(third);
    let out = ImageBuffer::new(first.width, first.height, PixelType::F32C3);
    parallel_rows::for_each(first.height, first.width * 3, |y| {
        for x in 0..first.width {
            for (c, source) in [first, second, third].iter().enumerate() {
                out.set_float(y, x * 3 + c, source.float(y, x));
            }
        }
    });
    out
}
pub fn spread(single: &ImageBuffer) -> ImageBuffer {
    merge(single, single, single)
}
pub fn tile(source: &ImageBuffer, down: usize, across: usize) -> ImageBuffer {
    let out = ImageBuffer::new(
        source.width * across,
        source.height * down,
        source.pixel_type,
    );
    for y in 0..out.height {
        for x in 0..out.width * source.channels() {
            copy(
                &out,
                y,
                x,
                source.value(y % source.height, x % (source.width * source.channels())),
            );
        }
    }
    out
}
pub fn pad_replicate(
    source: &ImageBuffer,
    top: usize,
    bottom: usize,
    left: usize,
    right: usize,
) -> ImageBuffer {
    assert!(!source.is_empty());
    let out = ImageBuffer::new(
        source.width + left + right,
        source.height + top + bottom,
        source.pixel_type,
    );
    for y in 0..out.height {
        for x in 0..out.width {
            for c in 0..source.channels() {
                copy(
                    &out,
                    y,
                    x * source.channels() + c,
                    source.value(
                        y.saturating_sub(top).min(source.height - 1),
                        x.saturating_sub(left).min(source.width - 1) * source.channels() + c,
                    ),
                );
            }
        }
    }
    out
}
pub fn crop(source: &ImageBuffer, x: usize, y: usize, width: usize, height: usize) -> ImageBuffer {
    source.view(x, y, width, height).isolated()
}

fn copy(out: &ImageBuffer, y: usize, x: usize, value: f32) {
    if out.pixel_type.is_float() {
        out.set_float(y, x, value);
    } else {
        out.set_byte(y, x, value as u8);
    }
}
