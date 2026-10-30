use super::{
    vote::DominantColorVote,
    window::{contains, distance, PixelWindow},
};
use crate::screen::Rect;

const NEAR: i32 = 75;
const PLATE_SHARE: f64 = 0.5;
const PLATE_RING: f64 = 0.1;
const ENCLOSURE: f64 = 0.3;
const APART: i32 = 150;
const ENCLOSED_WITH_CONTRAST: f64 = 0.3;
const ENCLOSED_ALONE: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextLayers {
    pub surface: [u8; 3],
    pub body: Option<[u8; 3]>,
    pub outline: Option<[u8; 3]>,
}

pub fn read(
    window: &PixelWindow<'_>,
    inner: Rect,
    surface_override: Option<[u8; 3]>,
) -> TextLayers {
    let ring = surface_override.unwrap_or_else(|| dominant_background(window, inner));
    let Some(first) = dominant_glyph_color(window, inner, ring) else {
        return TextLayers {
            surface: ring,
            body: None,
            outline: None,
        };
    };
    if let Some(body) = plate(window, inner, first) {
        return TextLayers {
            surface: first,
            body: Some(body),
            outline: None,
        };
    }
    if let Some(inside) = enclosed(window, inner, ring, first) {
        TextLayers {
            surface: ring,
            body: Some(inside),
            outline: Some(first),
        }
    } else {
        TextLayers {
            surface: ring,
            body: Some(first),
            outline: None,
        }
    }
}

fn plate(window: &PixelWindow<'_>, inner: Rect, first: [u8; 3]) -> Option<[u8; 3]> {
    let (mut covered, mut total, mut around, mut ring) = (0u64, 0u64, 0u64, 0u64);
    let outer = window.area;
    for y in outer.y..outer.bottom() {
        for x in (outer.x..outer.right()).step_by(2) {
            let near = u64::from(window.distance(x, y, first) < NEAR);
            if contains(inner, x, y) {
                covered += near;
                total += 1;
            } else {
                around += near;
                ring += 1;
            }
        }
    }
    if total == 0
        || (covered as f64) < total as f64 * PLATE_SHARE
        || (around as f64) < ring as f64 * PLATE_RING
    {
        None
    } else {
        dominant_glyph_color(window, inner, first)
    }
}

fn enclosed(
    window: &PixelWindow<'_>,
    inner: Rect,
    surface: [u8; 3],
    outline: [u8; 3],
) -> Option<[u8; 3]> {
    let outer = window.area;
    let (width, height) = (outer.w as usize, outer.h as usize);
    let mut other = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            other[y * width + x] =
                window.distance(outer.x + x as i32, outer.y + y as i32, outline) >= NEAR;
        }
    }
    let mut reached = vec![false; other.len()];
    let mut queue = Vec::with_capacity(other.len());
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if !other[i] || contains(inner, outer.x + x as i32, outer.y + y as i32) {
                continue;
            }
            reached[i] = true;
            queue.push(i);
        }
    }
    let mut head = 0;
    while head < queue.len() {
        let i = queue[head];
        head += 1;
        let x = i % width;
        let neighbors = [
            (x > 0).then(|| i - 1),
            (x < width - 1).then_some(i + 1),
            (i >= width).then(|| i - width),
            (i + width < other.len()).then_some(i + width),
        ];
        for next in neighbors.into_iter().flatten() {
            if other[next] && !reached[next] {
                reached[next] = true;
                queue.push(next);
            }
        }
    }
    let (mut outlined, mut enclosed) = (0u64, 0u64);
    let mut vote = DominantColorVote::default();
    for y in inner.y..inner.bottom() {
        for x in inner.x..inner.right() {
            let i = (y - outer.y) as usize * width + (x - outer.x) as usize;
            if !other[i] {
                outlined += 1;
            } else if !reached[i] {
                enclosed += 1;
                vote.add(window.at(x, y));
            }
        }
    }
    let body = vote.dominant()?;
    if outlined == 0
        || (enclosed as f64) < outlined as f64 * ENCLOSURE
        || distance(body, outline) < APART
    {
        return None;
    }
    let (mut body_like, mut body_enclosed) = (0u64, 0u64);
    for y in inner.y..inner.bottom() {
        for x in inner.x..inner.right() {
            if window.distance(x, y, body) >= NEAR {
                continue;
            }
            body_like += 1;
            let i = (y - outer.y) as usize * width + (x - outer.x) as usize;
            if other[i] && !reached[i] {
                body_enclosed += 1;
            }
        }
    }
    let share = if body_like == 0 {
        0.0
    } else {
        body_enclosed as f64 / body_like as f64
    };
    let stands_out = distance(body, surface) >= APART;
    (share
        >= if stands_out {
            ENCLOSED_WITH_CONTRAST
        } else {
            ENCLOSED_ALONE
        })
    .then_some(body)
}

fn dominant_background(window: &PixelWindow<'_>, inner: Rect) -> [u8; 3] {
    let mut buckets = vec![[0u64; 4]; 4096];
    let mut order = Vec::new();
    let outer = window.area;
    for y in outer.y..outer.bottom() {
        for x in (outer.x..outer.right()).step_by(2) {
            if contains(inner, x, y) {
                continue;
            }
            let c = window.at(x, y);
            let key = ((usize::from(c[0] >> 4) << 8) | (usize::from(c[1] >> 4) << 4))
                | usize::from(c[2] >> 4);
            let bucket = &mut buckets[key];
            if bucket[3] == 0 {
                order.push(key);
            }
            for i in 0..3 {
                bucket[i] += u64::from(c[i]);
            }
            bucket[3] += 1;
        }
    }
    let mut best = None;
    for key in order {
        if best.is_none_or(|old: usize| buckets[key][3] > buckets[old][3]) {
            best = Some(key);
        }
    }
    best.map_or([255; 3], |key| {
        let b = buckets[key];
        [
            (b[0] / b[3]) as u8,
            (b[1] / b[3]) as u8,
            (b[2] / b[3]) as u8,
        ]
    })
}

fn dominant_glyph_color(
    window: &PixelWindow<'_>,
    inner: Rect,
    background: [u8; 3],
) -> Option<[u8; 3]> {
    let mut max_diff = 0;
    for y in inner.y..inner.bottom() {
        for x in (inner.x..inner.right()).step_by(2) {
            max_diff = max_diff.max(window.distance(x, y, background));
        }
    }
    let threshold = 60.max((f64::from(max_diff) * 0.6) as i32);
    let mut vote = DominantColorVote::default();
    for y in inner.y..inner.bottom() {
        for x in (inner.x..inner.right()).step_by(2) {
            let c = window.at(x, y);
            if distance(c, background) >= threshold {
                vote.add(c);
            }
        }
    }
    vote.dominant()
}
