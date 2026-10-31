//! Detects columns read twice, once upright and once on the turned frame, and keeps one reading.

use super::super::{
    geom::RectF,
    line::{glyphs_in, says_the_same, squeeze, Line},
};

/// How much of the smaller box must lie inside the larger. Not full containment, since the two come
/// from separate expansions of one blob.
const INSIDE: f64 = 0.8;
/// How much of the smaller box two readings share before they are in the same place.
const SHARES_THE_PLACE: f64 = 0.3;
/// How much of a turned box the upright pass must already cover before the second reading is the
/// same writing. The pass exists for writing the upright one missed.
const SAME_PIECE_OF_WRITING: f64 = 0.65;

pub fn drop_repeated(columns: Vec<Line>) -> Vec<Line> {
    let mut repeated = vec![false; columns.len()];
    for i in 0..columns.len() {
        for j in 0..columns.len() {
            if i == j || repeated[i] || repeated[j] {
                continue;
            }
            let (a, b) = (&columns[i], &columns[j]);
            if a.line_geometry.is_some_and(|l| l.is_tilted())
                || b.line_geometry.is_some_and(|l| l.is_tilted())
            {
                continue;
            }
            let inner = a.layout_bounds;
            let shared = inner.intersect(b.layout_bounds);
            let area = inner.width * inner.height;
            if shared.is_empty()
                || area <= 0.0
                || shared.width * shared.height / area < INSIDE
                || !says_the_same(&a.text, &b.text)
            {
                continue;
            }
            let order = squeeze(&a.text)
                .encode_utf16()
                .count()
                .cmp(&squeeze(&b.text).encode_utf16().count());
            let less = if order.is_eq() {
                a.confidence.unwrap_or(0.0) < b.confidence.unwrap_or(0.0)
            } else {
                order.is_lt()
            };
            repeated[if less { i } else { j }] = true;
        }
    }
    columns
        .into_iter()
        .enumerate()
        .filter_map(|(i, b)| (!repeated[i]).then_some(b))
        .collect()
}

pub fn worth_keeping(block: &Line) -> bool {
    glyphs_in(&block.text) >= 2
}

pub fn overlap(a: RectF, b: RectF) -> f64 {
    let shared = a.intersect(b);
    if shared.is_empty() {
        return 0.0;
    }
    let smaller = (a.width * a.height).min(b.width * b.height);
    if smaller <= 0.0 {
        0.0
    } else {
        shared.width * shared.height / smaller
    }
}

pub fn says_what_was_already_read(candidate: &Line, upright: &[Line]) -> bool {
    !squeeze(&candidate.text).is_empty()
        && upright.iter().any(|block| {
            squeeze(&block.text).encode_utf16().count() >= 2
                && overlap(candidate.bounds, block.bounds) > SHARES_THE_PLACE
                && says_the_same(&candidate.text, &block.text)
        })
}

pub fn pieces_the_upright_pass_missed(
    upright: &[RectF],
    turned: &[RectF],
    source_width: u32,
) -> Vec<usize> {
    let mut taken = upright.to_vec();
    let mut missed = Vec::new();
    for (i, box_rect) in turned.iter().enumerate() {
        let candidate = super::geometry::to_upright(*box_rect, source_width);
        if already_read(candidate, &taken) > SAME_PIECE_OF_WRITING {
            continue;
        }
        taken.push(candidate);
        missed.push(i);
    }
    missed
}

fn already_read(candidate: RectF, read: &[RectF]) -> f64 {
    let area = candidate.width * candidate.height;
    if area <= 0.0 {
        return 0.0;
    }
    let pieces: Vec<_> = read
        .iter()
        .map(|b| candidate.intersect(*b))
        .filter(|p| !p.is_empty() && p.width > 0.0 && p.height > 0.0)
        .collect();
    let coordinates = |horizontal: bool| {
        let mut values: Vec<_> = pieces
            .iter()
            .flat_map(|p| {
                if horizontal {
                    [p.x, p.right()]
                } else {
                    [p.y, p.bottom()]
                }
            })
            .collect();
        values.sort_by(f64::total_cmp);
        values.dedup();
        values
    };
    let xs = coordinates(true);
    let ys = coordinates(false);
    let mut covered = 0.0;
    for x in xs.windows(2) {
        for y in ys.windows(2) {
            let middle = super::super::geom::PointF::new((x[0] + x[1]) / 2.0, (y[0] + y[1]) / 2.0);
            if pieces.iter().any(|p| p.contains(middle)) {
                covered += (x[1] - x[0]) * (y[1] - y[0]);
            }
        }
    }
    covered / area
}
