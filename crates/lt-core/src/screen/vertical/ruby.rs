//! Sets aside reading columns printed beside the writing they annotate.

use super::super::line::{glyphs_in, is_whitespace, Line};

/// How small a reading's glyphs must be against the writing's. Glyph size separates readings from
/// dialogue better than width does.
const SMALLER_THAN_THE_WRITING: f64 = 0.62;
/// How far a reading may sit from the writing, as a share of the writing's width. Generous in both
/// directions: expanded boxes overlap as often as they leave a gap.
const AGAINST_THE_WRITING: f64 = 0.5;
/// How much of the reading must lie alongside the writing.
const ALONGSIDE_THE_WRITING: f64 = 0.85;

pub fn separate(columns: Vec<Line>) -> (Vec<Line>, Vec<Line>) {
    let mut annotates = vec![None; columns.len()];
    let mut glosses = annotates.clone();
    for i in 0..columns.len() {
        for j in 0..columns.len() {
            if i == j {
                continue;
            }
            let (a, b) = (&columns[i], &columns[j]);
            let kanji = b
                .text
                .encode_utf16()
                .any(|c| matches!(c,0x4e00..=0x9fff | 0x3400..=0x4dbf));
            if !kanji || !sits_as_reading_of(a, b) {
                continue;
            }
            let dest = if is_kana_only(&a.text) {
                &mut annotates[i]
            } else if a.layout_bounds.height > a.layout_bounds.width
                && b.layout_bounds.height > b.layout_bounds.width
            {
                &mut glosses[i]
            } else {
                continue;
            };
            if dest.is_none_or(|chosen: usize| {
                b.layout_bounds.right() > columns[chosen].layout_bounds.right()
            }) {
                *dest = Some(j);
            }
        }
        if annotates[i].is_some() {
            glosses[i] = None;
        }
    }
    let mut bounds: Vec<_> = columns.iter().map(|c| c.bounds).collect();
    let mut layout: Vec<_> = columns.iter().map(|c| c.layout_bounds).collect();
    for i in 0..columns.len() {
        if let Some(j) = annotates[i] {
            bounds[j] = bounds[j].union(columns[i].bounds);
            layout[j] = layout[j].union(columns[i].layout_bounds);
        } else if let Some(j) = glosses[i] {
            layout[j] = layout[j].union(columns[i].layout_bounds);
        }
    }
    let (mut writing, mut readings) = (Vec::new(), Vec::new());
    for (i, mut column) in columns.into_iter().enumerate() {
        if annotates[i].is_some() {
            continue;
        }
        if glosses[i].is_some() {
            readings.push(column);
        } else {
            column.bounds = bounds[i];
            column.layout_bounds = layout[i];
            writing.push(column);
        }
    }
    (writing, readings)
}

fn glyph_size(column: &Line) -> f64 {
    column
        .layout_bounds
        .width
        .min(column.layout_bounds.height / glyphs_in(&column.text).max(1) as f64)
}
fn sits_as_reading_of(reading: &Line, writing: &Line) -> bool {
    let written = glyph_size(writing);
    if written <= 0.0 || glyph_size(reading) > written * SMALLER_THAN_THE_WRITING {
        return false;
    }
    let (a, b) = (reading.layout_bounds, writing.layout_bounds);
    if b.width <= 0.0 {
        return false;
    }
    let gap = a.x - b.right();
    if gap > b.width * AGAINST_THE_WRITING || gap < -b.width * AGAINST_THE_WRITING {
        return false;
    }
    a.height > 0.0 && a.bottom().min(b.bottom()) - a.y.max(b.y) >= a.height * ALONGSIDE_THE_WRITING
}
fn is_kana_only(text: &str) -> bool {
    let chars: Vec<_> = text.encode_utf16().filter(|c| !is_whitespace(*c)).collect();
    !chars.is_empty() && chars.into_iter().all(|c| matches!(c, 0x3041..=0x30ff))
}
