//! Text colors tuned for legibility on a background.

/// The least contrast text is drawn at. Below the usual body-text bar on purpose: that bar pushes
/// most colored text to black or white, and keeping the source color is why it is sampled.
pub const MINIMUM_CONTRAST: f64 = 3.0;
pub fn tune(text: [u8; 3], background: [u8; 3]) -> [u8; 3] {
    let bg_lum = perceived_luminance(background);
    let text_lum = perceived_luminance(text);
    let (h, s, l) = rgb_to_hsl(text);
    if s < 0.18 {
        if text_lum < 0.16 {
            return text;
        }
        if bg_lum >= 0.55 {
            let max_allowed_lum = 0.08f64.max(bg_lum - 0.24);
            let boosted_lum = max_allowed_lum.min(text_lum + 0.12);
            let boosted_lightness = 0.74f64.min(l + 0.1);
            return hsl_to_rgb(h, 0.22f64.min(s * 1.08), boosted_lightness.max(boosted_lum));
        }
        let min_allowed_lum = 0.92f64.min(bg_lum + 0.3);
        let lifted_lum = min_allowed_lum.max(text_lum + 0.08);
        let lifted_lightness = l.max(0.9f64.min(l + 0.08));
        return hsl_to_rgb(h, 0.22f64.min(s * 1.08), lifted_lightness.max(lifted_lum));
    }
    let target_saturation = 1.0f64.min((s + 0.08).max(s * 1.12));
    if bg_lum >= 0.55 {
        let max_allowed_lum = 0.08f64.max(bg_lum - 0.24);
        let adjusted_lum = max_allowed_lum.min(text_lum + 0.01);
        let adjusted_lightness = 0.64f64.min(l.max(l + 0.01));
        return hsl_to_rgb(h, target_saturation, adjusted_lightness.max(adjusted_lum));
    }
    let min_allowed_color_lum = 0.9f64.min(bg_lum + 0.22);
    let color_lum = min_allowed_color_lum.max(text_lum + 0.01);
    let color_lightness = l.max(0.8f64.min(l + 0.01));
    hsl_to_rgb(h, target_saturation, color_lightness.max(color_lum))
}
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
