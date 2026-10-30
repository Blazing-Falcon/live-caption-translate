pub const MINIMUM_CONTRAST: f64 = 3.0;
fn linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
pub fn relative_luminance(rgb: [u8; 3]) -> f64 {
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}
pub fn perceived_luminance(rgb: [u8; 3]) -> f64 {
    (0.299 * f64::from(rgb[0]) + 0.587 * f64::from(rgb[1]) + 0.114 * f64::from(rgb[2])) / 255.0
}
pub fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f64 {
    let x = relative_luminance(a);
    let y = relative_luminance(b);
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}
pub fn rgb_to_hsl(color: [u8; 3]) -> (f64, f64, f64) {
    let r = f64::from(color[0]) / 255.0;
    let g = f64::from(color[1]) / 255.0;
    let b = f64::from(color[2]) / 255.0;
    let max = r.max(g.max(b));
    let min = r.min(g.min(b));
    let l = (max + min) / 2.0;
    if (max - min).abs() < f64::from_bits(1) {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < f64::from_bits(1) {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() < f64::from_bits(1) {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, s, l)
}
fn hue_to_rgb(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [u8; 3] {
    let h = h - h.floor();
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    if s <= 0.0 {
        return [(l * 255.0).round_ties_even() as u8; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    [
        (hue_to_rgb(p, q, h + 1.0 / 3.0) * 255.0).round_ties_even() as u8,
        (hue_to_rgb(p, q, h) * 255.0).round_ties_even() as u8,
        (hue_to_rgb(p, q, h - 1.0 / 3.0) * 255.0).round_ties_even() as u8,
    ]
}
pub fn ensure_contrast(text: [u8; 3], background: [u8; 3], min_ratio: f64) -> [u8; 3] {
    if contrast_ratio(text, background) >= min_ratio {
        return text;
    }
    let (h, s, l) = rgb_to_hsl(text);
    let lighter_first = relative_luminance(text) >= relative_luminance(background);
    for lighter in [lighter_first, !lighter_first] {
        let limit = if lighter { 1.0 } else { 0.0 };
        if contrast_ratio(hsl_to_rgb(h, s, limit), background) < min_ratio {
            continue;
        }
        let (mut near, mut far) = (l, limit);
        for _ in 0..16 {
            let mid = (near + far) / 2.0;
            if contrast_ratio(hsl_to_rgb(h, s, mid), background) >= min_ratio {
                far = mid;
            } else {
                near = mid;
            }
        }
        return hsl_to_rgb(h, s, far);
    }
    if contrast_ratio([0; 3], background) >= contrast_ratio([255; 3], background) {
        [0; 3]
    } else {
        [255; 3]
    }
}
