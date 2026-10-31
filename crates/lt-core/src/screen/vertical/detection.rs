use super::super::{detection::Detection, ImageView};
use super::probability::DetectorProbability;

pub fn bounds(points: &[(i32, i32); 4]) -> [i32; 4] {
    [
        points.iter().map(|p| p.0).min().unwrap(),
        points.iter().map(|p| p.1).min().unwrap(),
        points.iter().map(|p| p.0).max().unwrap(),
        points.iter().map(|p| p.1).max().unwrap(),
    ]
}
fn clipped(image: &ImageView<'_>, points: &[(i32, i32); 4]) -> [i32; 4] {
    let [l, t, r, b] = bounds(points);
    [
        l.max(0),
        t.max(0),
        r.min(image.width as i32),
        b.min(image.height as i32),
    ]
}
fn upright(p: &[(i32, i32); 4], width: i32, height: i32) -> bool {
    f64::from(height) >= f64::from(width) * 1.5
        && (p[0].0 - p[3].0).abs() <= 3
        && (p[1].0 - p[2].0).abs() <= 3
        && (p[0].1 - p[1].1).abs() <= 3
        && (p[3].1 - p[2].1).abs() <= 3
}
fn rectangle(b: [i32; 4], score: f32) -> Detection {
    let [l, t, r, b] = b;
    Detection {
        points: [(l, t), (r, t), (r, b), (l, b)],
        score,
    }
}
fn luminance(image: &ImageView<'_>, x: i32, y: i32) -> i32 {
    let p = &image.row(y as u32)[x as usize * 4..];
    (i32::from(p[2]) * 77 + i32::from(p[1]) * 150 + i32::from(p[0]) * 29) >> 8
}
fn overlap(b: &Detection, part: [i32; 4]) -> f64 {
    let [left, top, right, bottom] = part;
    let [l, t, r, b] = bounds(&b.points);
    f64::from((right.min(r) - left.max(l)).max(0)) * f64::from((bottom.min(b) - top.max(t)).max(0))
        / f64::from((right - left) * (bottom - top))
}
fn split_on_probability(
    boxes: &[Detection],
    i: usize,
    probability: Option<&DetectorProbability<'_>>,
    area: [i32; 4],
) -> Vec<Detection> {
    let [l, t, r, b] = area;
    if let Some(probability) = probability {
        let columns = probability.columns_in(l, t, r, b);
        if columns.len() >= 2 {
            let parts: Vec<_> = columns
                .into_iter()
                .filter(|c| {
                    !boxes
                        .iter()
                        .enumerate()
                        .any(|(j, other)| i != j && overlap(other, c.kernel) > 0.65)
                })
                .map(|c| rectangle(c.bounds, boxes[i].score))
                .collect();
            if !parts.is_empty() {
                return parts;
            }
        }
    }
    vec![boxes[i].clone()]
}
fn ink_rows(image: &ImageView<'_>, area: [i32; 4], mode: i32) -> (i32, i32) {
    let [l, t, r, b] = area;
    let mut first = None;
    let mut last = 0;
    for y in t..b {
        if (l..r).any(|x| (luminance(image, x, y) - mode).abs() > 48) {
            first.get_or_insert(y);
            last = y;
        }
    }
    first.map_or((t, b - 1), |f| (f, last))
}
pub fn split(
    image: &ImageView<'_>,
    boxes: &[Detection],
    probability: Option<&DetectorProbability<'_>>,
) -> Vec<Detection> {
    let mut result = Vec::new();
    for (i, box_rect) in boxes.iter().enumerate() {
        let area = clipped(image, &box_rect.points);
        let [left, top, right, bottom] = area;
        let (width, height) = (right - left, bottom - top);
        if width < 16 {
            result.push(box_rect.clone());
            continue;
        }
        let fallback = || split_on_probability(boxes, i, probability, area);
        if !upright(&box_rect.points, width, height) {
            result.extend(fallback());
            continue;
        }
        let mut histogram = [0; 256];
        for y in top..bottom {
            for x in left..right {
                histogram[(luminance(image, x, y) / 16 * 16) as usize] += 1;
            }
        }
        let peak = *histogram.iter().max().unwrap();
        let mode = histogram.iter().position(|n| *n == peak).unwrap() as i32 + 8;
        if f64::from(peak) < f64::from(width * height) * 0.6 {
            result.extend(fallback());
            continue;
        }
        let mut ink = vec![0; width as usize];
        for y in top..bottom {
            for x in left..right {
                if (luminance(image, x, y) - mode).abs() > 48 {
                    ink[(x - left) as usize] += 1;
                }
            }
        }
        let on = |x: i32| f64::from(ink[x as usize]) > f64::from(height) * 0.015;
        let mut cuts = vec![0];
        let mut x = 0;
        while x < width {
            if on(x) {
                x += 1;
                continue;
            }
            let start = x;
            while x + 1 < width && !on(x + 1) {
                x += 1;
            }
            let end = x + 1;
            if end - start >= 2 && start >= 8 && width - end >= 8 {
                cuts.push((start + end) / 2);
            }
            x += 1;
        }
        if cuts.len() == 1 {
            result.extend(fallback());
            continue;
        }
        cuts.push(width);
        let whole = ink_rows(image, area, mode);
        let mut parts = Vec::new();
        for edges in cuts.windows(2) {
            let occupied: Vec<_> = (edges[0]..edges[1]).filter(|x| on(*x)).collect();
            if occupied.is_empty() {
                continue;
            }
            let (mut start, mut weight) = (occupied[0], 0_i64);
            let mut best = (occupied[0], occupied[0], 0, 0_i64);
            for (k, &x) in occupied.iter().enumerate() {
                weight += i64::from(ink[x as usize]);
                if k + 1 < occupied.len() && occupied[k + 1] - x <= 2 {
                    continue;
                }
                let w = x - start + 1;
                if w > best.2 || (w == best.2 && weight > best.3) {
                    best = (start, x, w, weight);
                }
                if k + 1 < occupied.len() {
                    start = occupied[k + 1];
                }
                weight = 0;
            }
            let (a, b) = (edges[0].max(best.0 - 2), edges[1].min(best.1 + 3));
            let (first, last) = ink_rows(image, [left + a, top, left + b, bottom], mode);
            let part = [
                left + a,
                top.max(first - (whole.0 - top)),
                left + b,
                bottom.min(last + 1 + (bottom - 1 - whole.1)),
            ];
            if boxes
                .iter()
                .enumerate()
                .any(|(j, other)| i != j && overlap(other, part) > 0.65)
            {
                continue;
            }
            parts.push(rectangle(part, box_rect.score));
        }
        if parts.len() >= 2 {
            result.extend(parts);
        } else {
            result.extend(fallback());
        }
    }
    result
}

pub fn extend(image: &ImageView<'_>, box_rect: &Detection) -> Detection {
    const REACH: f64 = 0.5;
    const ROW_INK: f64 = 0.05;
    let [left, top, right, bottom] = clipped(image, &box_rect.points);
    let (width, height) = (right - left, bottom - top);
    if width < 8 || !upright(&box_rect.points, width, height) {
        return box_rect.clone();
    }
    let mut histogram = [0; 256];
    let mut samples = 0;
    for y in (top..bottom).step_by(2) {
        for x in (left..right).step_by(2) {
            histogram[(luminance(image, x, y) / 16 * 16) as usize] += 1;
            samples += 1;
        }
    }
    let peak = *histogram.iter().max().unwrap();
    let mode = histogram.iter().position(|n| *n == peak).unwrap() as i32 + 8;
    if f64::from(peak) < f64::from(samples) * 0.45 {
        return box_rect.clone();
    }
    let budget = (f64::from(width) * REACH) as i32;
    let floor = (f64::from(width) * ROW_INK).max(1.0);
    let ink = |x, y| (luminance(image, x, y) - mode).abs() > 48;
    let continues = |y| {
        let count = (left..right).filter(|x| ink(*x, y)).count();
        let crosses = |edge: i32| {
            ink(if edge == 0 { 0 } else { edge - 1 }, y)
                && ink((image.width as i32 - 1).min(edge), y)
        };
        count as f64 >= floor && !(crosses(left) && crosses(right))
    };
    let (mut new_top, mut new_bottom) = (top, bottom);
    while new_top > top - budget && new_top > 0 && continues(new_top - 1) {
        new_top -= 1;
    }
    while new_bottom < bottom + budget && new_bottom < image.height as i32 && continues(new_bottom)
    {
        new_bottom += 1;
    }
    if new_top == top && new_bottom == bottom {
        box_rect.clone()
    } else {
        rectangle([left, new_top, right, new_bottom], box_rect.score)
    }
}
