use super::{ImageBuffer, PixelType};
pub fn count_non_zero(image: &ImageBuffer) -> usize {
    assert_eq!(image.channels(), 1);
    let mut n = 0;
    for y in 0..image.height {
        for x in 0..image.width {
            if image.value(y, x) != 0.0 {
                n += 1;
            }
        }
    }
    n
}
pub fn bounding_rect(image: &ImageBuffer) -> (usize, usize, usize, usize) {
    assert_eq!(image.pixel_type, PixelType::U8C1);
    let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..image.height {
        for x in 0..image.width {
            if image.byte(y, x) != 0 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x);
                bottom = bottom.max(y);
            }
        }
    }
    if top == usize::MAX {
        (0, 0, 0, 0)
    } else {
        (left, top, right - left + 1, bottom - top + 1)
    }
}
pub fn min_max(image: &ImageBuffer, mask: Option<&ImageBuffer>) -> (f64, f64) {
    assert_eq!(image.channels(), 1);
    if let Some(mask) = mask {
        image.require_mask(mask);
    }
    let (mut least, mut most, mut any) = (f64::MAX, f64::MIN, false);
    for y in 0..image.height {
        for x in 0..image.width {
            if mask.is_some_and(|m| m.byte(y, x) == 0) {
                continue;
            }
            any = true;
            let v = f64::from(image.value(y, x));
            if v < least {
                least = v;
            }
            if v > most {
                most = v;
            }
        }
    }
    if any {
        (least, most)
    } else {
        (0.0, 0.0)
    }
}
pub fn mean(image: &ImageBuffer, mask: Option<&ImageBuffer>) -> f64 {
    assert_eq!(image.pixel_type, PixelType::U8C1);
    if let Some(mask) = mask {
        image.require_mask(mask);
    }
    let (mut sum, mut count) = (0u64, 0u64);
    for y in 0..image.height {
        for x in 0..image.width {
            if mask.is_none_or(|m| m.byte(y, x) != 0) {
                sum += u64::from(image.byte(y, x));
                count += 1;
            }
        }
    }
    sum as f64 * if count != 0 { 1.0 / count as f64 } else { 0.0 }
}
