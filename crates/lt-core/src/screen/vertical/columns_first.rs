//! Assigns detector blobs to columns before rows, reassigning heads and tails between neighbouring
//! columns.

use super::probability::{blobs, Blob};
use std::collections::{BTreeMap, BTreeSet};

/// A blob this many times wider than tall is writing across, or a row of heads.
const ROW_SHAPE: f64 = 1.6;
/// The widest gap, in characters, between two pieces of one column.
const PIECE_GAP: f64 = 1.0;
/// Blobs smaller than this, in map pixels, are specks.
const MINIMUM_BLOB: usize = 12;
/// Value given to a bridged gap: over the detector threshold, under the ink either side.
const BRIDGE: f32 = 0.5;
/// Columns kept clear between what a column took of a row and what is left of it.
const GUTTER: i32 = 3;

#[derive(Clone, Copy)]
struct Piece {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    row: usize,
    blob: usize,
}
impl Piece {
    fn width(self) -> i32 {
        self.right - self.left + 1
    }
    fn height(self) -> i32 {
        self.bottom - self.top + 1
    }
    fn glyph(self) -> f64 {
        f64::from(self.width()).max(f64::from(self.height()).min(1.2 * f64::from(self.width())))
    }
    fn across(self, left: i32, right: i32) -> i32 {
        self.right.min(right) - self.left.max(left) + 1
    }
}
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}
fn ink(labels: &[usize], width: usize, row: Blob, from: i32, to: i32) -> Option<(i32, i32)> {
    let (mut top, mut bottom) = (None, 0);
    for y in row.top..=row.bottom {
        for x in from..=to {
            if labels[y as usize * width + x as usize] == row.id {
                top.get_or_insert(y);
                bottom = y;
                break;
            }
        }
    }
    top.map(|t| (t, bottom))
}
fn is_column(members: &[Piece]) -> bool {
    let mut own: Vec<_> = members
        .iter()
        .filter(|m| m.row == 0)
        .map(|m| m.width())
        .collect();
    if own.is_empty() {
        return false;
    }
    own.sort();
    let length = members.iter().map(|m| m.bottom).max().unwrap()
        - members.iter().map(|m| m.top).min().unwrap()
        + 1;
    f64::from(length) >= 1.5 * f64::from(own[own.len() / 2])
}
fn bridge(map: &mut [f32], width: usize, x0: i32, x1: i32, y0: i32, y1: i32) {
    let quarter = (x1 - x0 + 1) / 4;
    for y in y0..=y1 {
        for x in x0 + quarter..=x1 - quarter {
            let i = y as usize * width + x as usize;
            map[i] = map[i].max(BRIDGE);
        }
    }
}
fn give_to_writing(
    map: &mut [f32],
    labels: &[usize],
    width: usize,
    blobs: &[Blob],
    row: Blob,
    range: (i32, i32),
    glyph: f64,
) -> bool {
    let (from, to) = range;
    let Some((top, bottom)) = ink(labels, width, row, from, to) else {
        return false;
    };
    for other in blobs {
        if other.id == row.id || f64::from(other.width()) >= ROW_SHAPE * f64::from(other.height()) {
            continue;
        }
        let (x0, x1) = (from.max(other.left), to.min(other.right));
        if f64::from(x1 - x0 + 1) < 0.5 * f64::from(to - from + 1) {
            continue;
        }
        let (y0, y1) = if other.top > bottom {
            (bottom + 1, other.top - 1)
        } else if other.bottom < top {
            (other.bottom + 1, top - 1)
        } else {
            continue;
        };
        if f64::from(y1 - y0 + 1) > glyph {
            continue;
        }
        bridge(map, width, x0, x1, y0, y1);
        return true;
    }
    false
}

pub fn carve(map: &mut [f32], width: usize, height: usize, threshold: f32) -> bool {
    let (mut blobs, labels) = blobs(
        map,
        width,
        height,
        threshold,
        [0, 0, width as i32, height as i32],
    );
    blobs.retain(|b| b.count >= MINIMUM_BLOB);
    let rows: Vec<_> = blobs
        .iter()
        .copied()
        .filter(|b| f64::from(b.width()) >= ROW_SHAPE * f64::from(b.height()))
        .collect();
    if rows.is_empty() {
        return false;
    }
    let mut pieces: Vec<_> = blobs
        .iter()
        .filter(|b| f64::from(b.width()) < ROW_SHAPE * f64::from(b.height()))
        .map(|b| Piece {
            left: b.left,
            top: b.top,
            right: b.right,
            bottom: b.bottom,
            row: 0,
            blob: b.id,
        })
        .collect();
    if pieces.is_empty() {
        return false;
    }
    for _ in 0..4 {
        let mut added = false;
        for &row in &rows {
            let mut i = 0;
            while i < pieces.len() {
                let piece = pieces[i];
                i += 1;
                if piece.row == row.id
                    || f64::from(piece.width()) < 0.7 * f64::from(row.height())
                    || f64::from(piece.across(row.left, row.right)) < 0.7 * f64::from(piece.width())
                {
                    continue;
                }
                let glyph = piece.glyph();
                if f64::from(row.height()) < 0.6 * glyph || f64::from(row.height()) > 1.5 * glyph {
                    continue;
                }
                let gap = if piece.top > row.bottom {
                    piece.top - row.bottom - 1
                } else {
                    row.top - piece.bottom - 1
                };
                if gap < 0 || f64::from(gap) > PIECE_GAP * glyph {
                    continue;
                }
                if pieces.iter().any(|p| {
                    p.row == row.id && f64::from(p.across(piece.left, piece.right)) > 0.5 * glyph
                }) {
                    continue;
                }
                let (left, right) = (piece.left.max(row.left), piece.right.min(row.right));
                let Some((top, bottom)) = ink(&labels, width, row, left, right) else {
                    continue;
                };
                pieces.push(Piece {
                    left,
                    top,
                    right,
                    bottom,
                    row: row.id,
                    blob: row.id,
                });
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    if pieces.iter().all(|p| p.row == 0) {
        return false;
    }
    let mut parent: Vec<_> = (0..pieces.len()).collect();
    for a in 0..pieces.len() {
        for b in a + 1..pieces.len() {
            let (p, q) = (pieces[a], pieces[b]);
            if p.row != 0 && p.row == q.row {
                continue;
            }
            if f64::from(p.across(q.left, q.right)) < 0.5 * f64::from(p.width().min(q.width())) {
                continue;
            }
            let gap = if p.top <= q.top {
                q.top - p.bottom - 1
            } else {
                p.top - q.bottom - 1
            };
            if f64::from(gap) > PIECE_GAP * p.glyph().max(q.glyph()) {
                continue;
            }
            let root_a = find(&mut parent, a);
            let root_b = find(&mut parent, b);
            parent[root_a] = root_b;
        }
    }
    let mut columns: BTreeMap<usize, Vec<Piece>> = BTreeMap::new();
    for (i, piece) in pieces.iter().enumerate() {
        columns
            .entry(find(&mut parent, i))
            .or_default()
            .push(*piece);
    }
    let standing: BTreeSet<_> = columns
        .values()
        .filter(|m| is_column(m))
        .flatten()
        .filter(|m| m.row == 0)
        .map(|m| m.blob)
        .collect();
    let (mut changed, mut took) = (false, BTreeSet::new());
    for row in rows {
        let given: Vec<_> = (0..pieces.len())
            .filter(|i| pieces[*i].row == row.id && is_column(&columns[&find(&mut parent, *i)]))
            .collect();
        if given.is_empty() {
            continue;
        }
        took.extend(given.iter().map(|i| find(&mut parent, *i)));
        let mut widths: Vec<_> = given.iter().map(|i| pieces[*i].width()).collect();
        widths.sort();
        let glyph = widths[given.len() / 2];
        let mut claimed = vec![false; row.width() as usize];
        for &i in &given {
            for x in pieces[i].left..=pieces[i].right {
                claimed[(x - row.left) as usize] = true;
            }
        }
        let mut start = 0;
        while start < claimed.len() {
            if claimed[start] {
                start += 1;
                continue;
            }
            let mut end = start;
            while end + 1 < claimed.len() && !claimed[end + 1] {
                end += 1;
            }
            let (from, to) = (row.left + start as i32, row.left + end as i32);
            if to - from + 1 < glyph
                || !give_to_writing(
                    map,
                    &labels,
                    width,
                    &blobs,
                    row,
                    (from, to),
                    f64::from(glyph),
                )
            {
                for y in row.top..=row.bottom {
                    for x in from.max(row.left)..=to.min(row.right) {
                        let i = y as usize * width + x as usize;
                        if labels[i] == row.id {
                            map[i] = 0.0;
                        }
                    }
                }
            }
            start = end + 1;
        }
        for i in given {
            let head = pieces[i];
            for y in (row.top - 1).max(0)..=(row.bottom + 1).min(height as i32 - 1) {
                for d in 1..=GUTTER {
                    for x in [head.left - d, head.right + d] {
                        if x >= 0 && x < width as i32 {
                            let i = y as usize * width + x as usize;
                            if !standing.contains(&labels[i]) {
                                map[i] = 0.0;
                            }
                        }
                    }
                }
            }
        }
        changed = true;
    }
    for root in took {
        let mut members = columns[&root].clone();
        members.sort_by_key(|m| m.top);
        for pair in members.windows(2) {
            let (above, below) = (pair[0], pair[1]);
            if below.top <= above.bottom + 1 {
                continue;
            }
            let (x0, x1) = (above.left.max(below.left), above.right.min(below.right));
            if x1 < x0 {
                continue;
            }
            bridge(map, width, x0, x1, above.bottom + 1, below.top - 1);
        }
    }
    changed
}
