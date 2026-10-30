use super::{saturate, ImageBuffer, PixelType};
const EPSILON: f64 = 1.1920928955078125e-7;
pub fn binary(source: &ImageBuffer, target: &ImageBuffer, threshold: f64, maximum: f64) {
    source.same_shape(target);
    assert_eq!(source.channels(), 1);
    for y in 0..source.height {
        for x in 0..source.width {
            if source.pixel_type == PixelType::U8C1 {
                let at = (threshold.floor() as i32).clamp(-1, 255);
                target.set_byte(
                    y,
                    x,
                    if i32::from(source.byte(y, x)) > at {
                        saturate::to_byte(maximum)
                    } else {
                        0
                    },
                );
            } else {
                target.set_float(
                    y,
                    x,
                    if source.float(y, x) > threshold as f32 {
                        maximum as f32
                    } else {
                        0.0
                    },
                );
            }
        }
    }
}
pub fn otsu(source: &ImageBuffer) -> f64 {
    assert_eq!(source.pixel_type, PixelType::U8C1);
    let mut histogram = [0i32; 256];
    for y in 0..source.height {
        for x in 0..source.width {
            histogram[source.byte(y, x) as usize] += 1;
        }
    }
    let scale = 1.0 / (source.width as f64 * source.height as f64);
    let mut mu = 0.0;
    for (i, h) in histogram.iter().enumerate() {
        mu += i as f64 * f64::from(*h);
    }
    mu *= scale;
    let (mut mu1, mut q1, mut max_sigma, mut max_value) = (0.0, 0.0, 0.0, 0.0);
    for (i, h) in histogram.iter().enumerate() {
        let p = f64::from(*h) * scale;
        mu1 *= q1;
        q1 += p;
        let q2 = 1.0 - q1;
        if q1.min(q2) < EPSILON || q1.max(q2) > 1.0 - EPSILON {
            continue;
        }
        mu1 = (mu1 + i as f64 * p) / q1;
        let mu2 = (mu - q1 * mu1) / q2;
        let sigma = q1 * q2 * (mu1 - mu2) * (mu1 - mu2);
        if sigma > max_sigma {
            max_sigma = sigma;
            max_value = i as f64;
        }
    }
    max_value
}
