//! Pixel type conversion with scaling and saturation.

use super::parallel_rows;
use super::{saturate, ImageBuffer, PixelType};
pub fn convert(source: &ImageBuffer, pixel_type: PixelType, scale: f64) -> ImageBuffer {
    assert_eq!(source.channels(), pixel_type.channels());
    let target = ImageBuffer::new(source.width, source.height, pixel_type);
    convert_to(source, &target, scale);
    target
}
pub fn convert_to(source: &ImageBuffer, target: &ImageBuffer, scale: f64) {
    assert_eq!(
        (source.width, source.height, source.channels()),
        (target.width, target.height, target.channels())
    );
    let s = scale as f32;
    parallel_rows::for_each(source.height, source.width * source.channels(), |y| {
        for x in 0..source.width * source.channels() {
            let v = if scale == 1.0 {
                source.value(y, x)
            } else {
                source.value(y, x) * s
            };
            if target.pixel_type.is_float() {
                target.set_float(y, x, v)
            } else {
                target.set_byte(y, x, saturate::to_byte(f64::from(v)))
            }
        }
    });
}
