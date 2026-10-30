use super::parallel_rows;
use super::{ImageBuffer, PixelType};
const BLUE_WEIGHT: i32 = 3735;
const GREEN_WEIGHT: i32 = 19235;
const RED_WEIGHT: i32 = 9798;
const SHIFT: i32 = 15;
const BLUE_WEIGHT_F: f32 = 0.114;
const GREEN_WEIGHT_F: f32 = 0.587;
const RED_WEIGHT_F: f32 = 0.299;
pub fn to_gray(source: &ImageBuffer) -> ImageBuffer {
    assert_eq!(source.channels(), 3);
    let target = ImageBuffer::new(
        source.width,
        source.height,
        source.pixel_type.with_channels(1),
    );
    parallel_rows::for_each(source.height, source.width * 3, |y| {
        for x in 0..source.width {
            if source.pixel_type == PixelType::U8C3 {
                target.set_byte(
                    y,
                    x,
                    ((i32::from(source.byte(y, x * 3)) * BLUE_WEIGHT
                        + i32::from(source.byte(y, x * 3 + 1)) * GREEN_WEIGHT
                        + i32::from(source.byte(y, x * 3 + 2)) * RED_WEIGHT
                        + (1 << (SHIFT - 1)))
                        >> SHIFT) as u8,
                )
            } else {
                target.set_float(
                    y,
                    x,
                    source.float(y, x * 3 + 2).mul_add(
                        RED_WEIGHT_F,
                        source
                            .float(y, x * 3)
                            .mul_add(BLUE_WEIGHT_F, source.float(y, x * 3 + 1) * GREEN_WEIGHT_F),
                    ),
                );
            }
        }
    });
    target
}
