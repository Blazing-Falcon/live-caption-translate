//! Joins the lines OCR finds into text blocks in reading order. Heights are those of the line
//! rects; every threshold is a multiple of the line height `h`, so it holds at any text size.

use super::{paint, Block, ImageView, Rect};
use crate::engines::TextLine;
use std::collections::BTreeMap;

/// Two lines are one line when they overlap vertically by this share of the smaller height and
/// the gap between them is at most `ROW_GAP` heights of the taller one.
const ROW_OVERLAP: f64 = 0.6;
const ROW_GAP: f64 = 0.8;

// A line continues the paragraph above it when
// - its height is within `HEIGHT_RATIO` of the height above;
// - it starts between `GAP_MIN` and `GAP_MAX` heights below it (at most `GAP_AFTER_SENTENCE` when
//   the line above ends a sentence, where a new item is as likely as a continuation);
// - its left edge is within `LEFT_EDGE` heights of the one above, or it lies over `COLUMN_OVERLAP`
//   of the narrower line;
// - the line above is at least `FULL_LINE` heights wide, unless it ends a sentence: a wrapped
//   line fills its column, while the cells of a table and the entries of a list are short.
const HEIGHT_RATIO: f64 = 1.35;
const GAP_MIN: f64 = -0.3;
const GAP_MAX: f64 = 0.7;
const GAP_AFTER_SENTENCE: f64 = 0.4;
const LEFT_EDGE: f64 = 1.5;
const COLUMN_OVERLAP: f64 = 0.5;
const FULL_LINE: f64 = 7.0;
/// A line indented by more than this many heights past the line above it starts a paragraph.
const INDENT: f64 = 1.0;
/// Blocks whose tops differ by less than this many heights of the first one read as one row.
const READING_ROW: f64 = 0.5;

/// Blocks for the lines OCR found in `image`, in reading order.
pub fn blocks(image: &ImageView<'_>, lines: &[TextLine]) -> Vec<Block> {
    let groups = group(lines);
    let every_line: Vec<Rect> = groups
        .iter()
        .flat_map(|group| group.lines.iter().copied())
        .collect();
    let surface = paint::Surface::new(image, &every_line);
    groups
        .into_iter()
        .enumerate()
        .map(|(id, group)| {
            let background = surface.background(&group.lines);
            Block {
                id: id as u32,
                rect: group.rect(),
                line_height: group.line_height(),
                translate: group.source.chars().any(is_han),
                foreground: paint::foreground(background),
                background,
                lines: group.lines,
                source: group.source,
            }
        })
        .collect()
}

struct Group {
    lines: Vec<Rect>,
    source: String,
}

impl Group {
    fn rect(&self) -> Rect {
        union(self.lines.iter())
    }

    fn line_height(&self) -> u32 {
        let mut heights: Vec<u32> = self.lines.iter().map(|line| line.h).collect();
        heights.sort_unstable();
        heights[heights.len() / 2]
    }
}

struct Line {
    rect: Rect,
    text: String,
}

fn group(lines: &[TextLine]) -> Vec<Group> {
    let mut lines = same_row(lines);
    lines.sort_by_key(|line| (line.rect.y, line.rect.x));
    let mut groups: Vec<Group> = Vec::new();
    for line in lines {
        let target = groups
            .iter_mut()
            .filter_map(|group| {
                let last = *group.lines.last()?;
                continues(&last, ends_sentence(&group.source), &line.rect).then(|| {
                    let gap = i64::from(line.rect.y) - i64::from(last.bottom());
                    let shift = i64::from(line.rect.x) - i64::from(last.x);
                    ((gap.abs(), shift.abs()), group)
                })
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, group)| group);
        match target {
            Some(group) => {
                let last = group.lines[group.lines.len() - 1];
                if starts_paragraph(&group.source, &last, &line) {
                    group.source.push('\n');
                } else {
                    group.source.push_str(separator(&group.source, &line.text));
                }
                group.source.push_str(&line.text);
                group.lines.push(line.rect);
            }
            None => groups.push(Group {
                lines: vec![line.rect],
                source: line.text,
            }),
        }
    }
    reading_order(groups)
}

/// Lines side by side on one row become one line, read left to right.
fn same_row(lines: &[TextLine]) -> Vec<Line> {
    let mut parent: Vec<usize> = (0..lines.len()).collect();
    fn root(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    for a in 0..lines.len() {
        for b in a + 1..lines.len() {
            if on_one_row(&lines[a].rect, &lines[b].rect) {
                let (a, b) = (root(&mut parent, a), root(&mut parent, b));
                parent[b.max(a)] = a.min(b);
            }
        }
    }
    let mut rows: BTreeMap<usize, Vec<&TextLine>> = BTreeMap::new();
    for (index, line) in lines.iter().enumerate() {
        rows.entry(root(&mut parent, index)).or_default().push(line);
    }
    rows.into_values()
        .map(|mut row| {
            row.sort_by_key(|line| line.rect.x);
            let mut text = String::new();
            for line in &row {
                text.push_str(separator(&text, &line.text));
                text.push_str(&line.text);
            }
            Line {
                rect: union(row.iter().map(|line| &line.rect)),
                text,
            }
        })
        .collect()
}

fn on_one_row(a: &Rect, b: &Rect) -> bool {
    let overlap = i64::from(a.bottom().min(b.bottom())) - i64::from(a.y.max(b.y));
    let gap = i64::from(a.x.max(b.x)) - i64::from(a.right().min(b.right()));
    overlap as f64 >= ROW_OVERLAP * f64::from(a.h.min(b.h))
        && gap as f64 <= ROW_GAP * f64::from(a.h.max(b.h))
}

/// `next` reads on from `last`, which ends a sentence if `ended`: the next line of the same
/// paragraph.
fn continues(last: &Rect, ended: bool, next: &Rect) -> bool {
    let (above, below) = (f64::from(last.h), f64::from(next.h));
    let h = (above + below) / 2.0;
    let gap = f64::from(next.y) - f64::from(last.bottom());
    let shift = (f64::from(next.x) - f64::from(last.x)).abs();
    let shared = i64::from(last.right().min(next.right())) - i64::from(last.x.max(next.x));
    let most = if ended { GAP_AFTER_SENTENCE } else { GAP_MAX };
    above.max(below) <= HEIGHT_RATIO * above.min(below)
        && (GAP_MIN * h..=most * h).contains(&gap)
        && (shift <= LEFT_EDGE * h
            || shared as f64 >= COLUMN_OVERLAP * f64::from(last.w.min(next.w)))
        && (ended || f64::from(last.w) >= FULL_LINE * above)
}

/// The line break stays where the line above ended a sentence or this line is indented.
fn starts_paragraph(text: &str, last: &Rect, next: &Line) -> bool {
    let h = (f64::from(last.h) + f64::from(next.rect.h)) / 2.0;
    let indent = f64::from(next.rect.x) - f64::from(last.x);
    ends_sentence(text) || indent > INDENT * h
}

fn ends_sentence(text: &str) -> bool {
    let end = text.trim_end_matches(['”', '’', '」', '』', '）', ')']);
    end.ends_with(['。', '！', '？'])
}

/// Blocks by top, then left; tops close together count as one row.
fn reading_order(mut groups: Vec<Group>) -> Vec<Group> {
    groups.sort_by_key(|group| {
        let rect = group.rect();
        (rect.y, rect.x)
    });
    let mut ordered: Vec<Group> = Vec::with_capacity(groups.len());
    let mut row: Vec<Group> = Vec::new();
    let mut row_top = 0.0;
    let mut row_height = 0.0;
    for group in groups {
        let top = f64::from(group.rect().y);
        if !row.is_empty() && top - row_top >= READING_ROW * row_height {
            flush(&mut row, &mut ordered);
        }
        if row.is_empty() {
            row_top = top;
            row_height = f64::from(group.lines[0].h);
        }
        row.push(group);
    }
    flush(&mut row, &mut ordered);
    ordered
}

fn flush(row: &mut Vec<Group>, ordered: &mut Vec<Group>) {
    row.sort_by_key(|group| group.rect().x);
    ordered.append(row);
}

fn union<'a>(rects: impl Iterator<Item = &'a Rect>) -> Rect {
    let (mut left, mut top, mut right, mut bottom) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for rect in rects {
        left = left.min(rect.x);
        top = top.min(rect.y);
        right = right.max(rect.right());
        bottom = bottom.max(rect.bottom());
    }
    Rect {
        x: left,
        y: top,
        w: right.abs_diff(left),
        h: bottom.abs_diff(top),
    }
}

/// What goes between two pieces of text: nothing next to Han characters and CJK punctuation,
/// which carry their own spacing, otherwise one space.
fn separator(before: &str, after: &str) -> &'static str {
    match (before.chars().next_back(), after.chars().next()) {
        (Some(a), Some(b)) if !is_cjk(a) && !is_cjk(b) => " ",
        _ => "",
    }
}

pub(super) fn is_han(c: char) -> bool {
    matches!(c,
        '\u{3007}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{2FA1F}')
}

fn is_cjk(c: char) -> bool {
    is_han(c) || matches!(c, '\u{3000}'..='\u{303F}' | '\u{FF00}'..='\u{FFEF}')
}
