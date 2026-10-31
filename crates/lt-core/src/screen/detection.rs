//! Turns the detector's probability map into scored quadrilaterals in reading order.

use super::contour::{mini_box, outlines, unclip, Point};

/// Probability above which a map pixel counts as text.
pub const BINARY: f32 = 0.2;
/// Boxes whose mean probability is lower are dropped as noise.
pub const BOX_SCORE: f32 = 0.4;
/// The detector is trained on shrunken text areas, so each box is grown by this ratio.
pub const UNCLIP: f32 = 1.4;
/// Boxes with a side shorter than this are specks, not text.
const MIN_SIDE: f32 = 3.0;
/// The mask is dilated by this many pixels before outlining, which joins characters of one word.
const DILATE_RADIUS: usize = 1;
/// Boxes whose tops differ by less than this many pixels are on one row when ordering.
const LINE_SORT_Y_THRESHOLD: i32 = 10;

#[derive(Clone, Debug)]
pub struct Detection {
    pub points: [(i32, i32); 4],
    pub score: f32,
}

pub fn boxes(map: &[f32], width: usize, height: usize) -> Vec<Detection> {
    let bytes: Vec<u8> = map
        .iter()
        .map(|v| (v * 255.0).round_ties_even() as u8)
        .collect();
    let mut mask = vec![0; width * height];
    for y in 0..height {
        for x in 0..width {
            mask[y * width + x] = i32::from(
                (y.saturating_sub(DILATE_RADIUS)..=(y + DILATE_RADIUS).min(height - 1)).any(|yy| {
                    (x.saturating_sub(DILATE_RADIUS)..=(x + DILATE_RADIUS).min(width - 1))
                        .any(|xx| map[yy * width + xx] > BINARY)
                }),
            );
        }
    }
    let mut boxes = Vec::new();
    for contour in outlines(mask, width, height) {
        if contour.len() <= 2 {
            continue;
        }
        let Some((rectangle, side)) = mini_box(&contour) else {
            continue;
        };
        if side.is_nan() || side < MIN_SIDE {
            continue;
        }
        let score = score(&contour, &bytes, width, height);
        if score < f64::from(BOX_SCORE) {
            continue;
        }
        let Some(expanded) = unclip(&rectangle, UNCLIP) else {
            continue;
        };
        let Some((rectangle, side)) = mini_box(&expanded) else {
            continue;
        };
        if side.is_nan() || side < MIN_SIDE + 2.0 {
            continue;
        }
        boxes.push(Detection {
            points: rectangle.map(|p| {
                (
                    (p.x as i32).clamp(0, width as i32),
                    (p.y as i32).clamp(0, height as i32),
                )
            }),
            score: score as f32,
        });
    }
    boxes.sort_by_key(|b| b.points[0].1);
    let mut lines = vec![0; boxes.len()];
    for i in 1..boxes.len() {
        lines[i] = lines[i - 1]
            + i32::from(boxes[i].points[0].1 - boxes[i - 1].points[0].1 >= LINE_SORT_Y_THRESHOLD);
    }
    let mut indexed: Vec<_> = boxes.into_iter().zip(lines).collect();
    indexed.sort_by_key(|(b, line)| (*line, b.points[0].0));
    indexed.into_iter().map(|(b, _)| b).collect()
}

fn score(contour: &[Point], bytes: &[u8], width: usize, height: usize) -> f64 {
    let xmin = contour.iter().map(|p| p.x as i32).min().unwrap();
    let xmax = contour.iter().map(|p| p.x as i32).max().unwrap();
    let ymin = contour.iter().map(|p| p.y as i32).min().unwrap();
    let ymax = contour.iter().map(|p| p.y as i32).max().unwrap();
    let (mut sum, mut count) = (0.0, 0);
    for y in ymin..=ymax {
        let center = y as f32 + 0.5;
        let mut intersections = Vec::new();
        for i in 0..contour.len() {
            let a = contour[i];
            let b = contour[(i + 1) % contour.len()];
            if (a.y <= center && b.y > center) || (b.y <= center && a.y > center) {
                intersections.push(a.x + (center - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
        intersections.sort_by(f32::total_cmp);
        for pair in intersections.as_chunks::<2>().0 {
            for x in (pair[0] + 0.5).floor() as i32..(pair[1] + 0.5).floor() as i32 {
                let sx = (xmin - 1).max(0) + x - xmin;
                let sy = (ymin - 1).max(0) + y - ymin;
                if sx < 0 || sx >= width as i32 || sy < 0 || sy >= height as i32 {
                    return 0.0;
                }
                if x > xmax {
                    continue;
                }
                sum += f64::from(bytes[sy as usize * width + sx as usize]);
                count += 1;
            }
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / f64::from(count) / 255.0
    }
}
