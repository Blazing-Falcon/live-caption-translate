use super::engine::Node;
use crate::screen::{
    geom::{max, min},
    line::Line,
};
use std::rc::Rc;

const SMALLER_THAN_THE_WRITING: f64 = 0.75;
const AGAINST_THE_WRITING: f64 = 0.45;
const ALONGSIDE_THE_WRITING: f64 = 0.85;

pub(super) fn drop(lines: Vec<Node>) -> Vec<Node> {
    if lines.len() < 2 {
        return lines;
    }
    let mut annotates = vec![None; lines.len()];
    let mut found = false;
    for i in 0..lines.len() {
        if !is_hiragana_only(&lines[i].text) {
            continue;
        }
        for j in 0..lines.len() {
            if i == j || !is_reading_of(&lines[i], &lines[j]) {
                continue;
            }
            if annotates[i].is_none_or(|chosen: usize| {
                lines[j].layout_bounds.y < lines[chosen].layout_bounds.y
            }) {
                annotates[i] = Some(j);
            }
            found = true;
        }
    }
    if !found {
        return lines;
    }
    let mut bounds: Vec<_> = lines.iter().map(|line| line.bounds).collect();
    for i in 0..lines.len() {
        if let Some(j) = annotates[i] {
            bounds[j] = bounds[j].union(lines[i].bounds);
        }
    }
    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| annotates[*i].is_none())
        .map(|(i, line)| {
            Rc::new(Line {
                bounds: bounds[i],
                ..(**line).clone()
            })
        })
        .collect()
}

pub fn drop_lines(lines: &[Line]) -> Vec<Line> {
    drop(lines.iter().cloned().map(Rc::new).collect())
        .iter()
        .map(|line| (**line).clone())
        .collect()
}

fn is_reading_of(reading: &Line, writing: &Line) -> bool {
    if !writing
        .text
        .encode_utf16()
        .any(|c| matches!(c,0x4e00..=0x9fff|0x3400..=0x4dbf))
    {
        return false;
    }
    let a = reading.layout_bounds;
    let b = writing.layout_bounds;
    if a.width <= 0.0 || a.height <= 0.0 || b.height <= 0.0 || b.width <= b.height {
        return false;
    }
    if a.height > b.height * SMALLER_THAN_THE_WRITING {
        return false;
    }
    let gap = b.y - a.bottom();
    if gap > 0.0 || gap < -b.height * AGAINST_THE_WRITING {
        return false;
    }
    let shared = min(a.right(), b.right()) - max(a.x, b.x);
    shared >= a.width * ALONGSIDE_THE_WRITING
}

fn is_hiragana_only(text: &str) -> bool {
    let mut any = false;
    for c in text.encode_utf16() {
        if crate::screen::line::is_whitespace(c) {
            continue;
        }
        if (0x3041..=0x3096).contains(&c) {
            any = true;
            continue;
        }
        if matches!(c, 0x30fc | 0x30fb | 0x309b | 0x309c | 0x309d | 0x309e) {
            continue;
        }
        return false;
    }
    any
}
