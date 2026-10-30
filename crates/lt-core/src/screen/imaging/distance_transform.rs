use super::{ImageBuffer, PixelType};
const STRAIGHT: f32 = 0.955;
const DIAGONAL: f32 = 1.3693;
pub fn chamfer3(mask: &ImageBuffer) -> ImageBuffer {
    assert_eq!(mask.pixel_type, PixelType::U8C1);
    let (w, h) = (mask.width, mask.height);
    let target = ImageBuffer::new(w, h, PixelType::F32C1);
    if mask.is_empty() {
        return target;
    }
    let step = w + 2;
    let mut t = vec![f32::MAX; step * (h + 2)];
    for y in 0..h {
        for x in 0..w {
            let i = (y + 1) * step + x + 1;
            if mask.byte(y, x) == 0 {
                t[i] = 0.0;
                continue;
            }
            let mut best = t[i - step - 1] + DIAGONAL;
            best = best.min(t[i - step] + STRAIGHT);
            best = best.min(t[i - step + 1] + DIAGONAL);
            best = best.min(t[i - 1] + STRAIGHT);
            t[i] = best;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = (y + 1) * step + x + 1;
            let mut best = t[i];
            best = best.min(t[i + step + 1] + DIAGONAL);
            best = best.min(t[i + step] + STRAIGHT);
            best = best.min(t[i + step - 1] + DIAGONAL);
            best = best.min(t[i + 1] + STRAIGHT);
            t[i] = best;
            target.set_float(y, x, best);
        }
    }
    target
}
