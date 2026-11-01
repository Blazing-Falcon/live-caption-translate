//! The grouping passes: ruby removal, same-row merging, next-line joining and trace collection.

use super::{
    next_line,
    profile::{GroupingProfile, GENERAL},
    rows, ruby,
    trace::{GroupingTrace, NextLineDecision},
};
use crate::screen::{
    geom::{max, min, LineGeometry, RectF},
    line::{self, LayoutScript, Line, TiltedLine},
};
use serde::Serialize;
use std::rc::Rc;

pub(super) type Node = Rc<Line>;

#[derive(Debug, Serialize)]
pub struct GroupingReport {
    pub groups: Vec<Line>,
    pub decisions: Vec<NextLineDecision>,
    pub trace: GroupingTrace,
}

pub(super) struct Context {
    pub collect: bool,
    pub decisions: Vec<NextLineDecision>,
    pub trace: GroupingTrace,
}

pub fn group(lines: &[Line]) -> Vec<Line> {
    group_with_profile(lines, &GENERAL, false).groups
}

pub fn group_with_profile(
    lines: &[Line],
    profile: &GroupingProfile,
    explain: bool,
) -> GroupingReport {
    let mut context = Context {
        collect: explain,
        decisions: Vec::new(),
        trace: GroupingTrace::default(),
    };
    let blocks: Vec<Node> = lines.iter().cloned().map(Rc::new).collect();
    for block in &blocks {
        debug_assert!(
            block.layout_bounds.width > 0.0 && block.layout_bounds.height > 0.0,
            "Block {:?} has no layout bounds",
            block.text
        );
    }
    if explain {
        context.trace.register_blocks(&blocks);
    }
    let groups = if blocks.len() <= 1 {
        if explain {
            for block in &blocks {
                context
                    .trace
                    .register_line(block, std::slice::from_ref(block));
            }
            context.trace.order_lines(&blocks);
            let groups: Vec<_> = blocks.iter().cloned().map(|b| vec![b]).collect();
            context.trace.register_groups(&groups);
        }
        lines.to_vec()
    } else {
        let merged = rows::merge_same_line_fragments(&blocks, profile, &mut context);
        let mut sorted = ruby::drop(merged);
        sorted.sort_by(|a, b| {
            a.layout_bounds
                .y
                .total_cmp(&b.layout_bounds.y)
                .then(a.layout_bounds.x.total_cmp(&b.layout_bounds.x))
        });
        if explain {
            context.trace.order_lines(&sorted);
        }
        let mut groups: Vec<Vec<Node>> = Vec::new();
        for block in &sorted {
            if let Some(index) =
                next_line::group_this_line_continues(&groups, block, &sorted, profile, &mut context)
            {
                groups[index].push(Rc::clone(block));
            } else {
                groups.push(vec![Rc::clone(block)]);
            }
        }
        if explain {
            context.trace.register_groups(&groups);
        }
        groups
            .iter()
            .map(|group| build_group(&group.iter().map(|b| (**b).clone()).collect::<Vec<_>>()))
            .collect()
    };
    GroupingReport {
        groups,
        decisions: context.decisions,
        trace: context.trace,
    }
}

pub fn join_inline_text(left: &str, right: &str) -> String {
    let left = left.trim_end();
    let right = right.trim_start();
    if left.is_empty() {
        return right.into();
    }
    if right.is_empty() {
        return left.into();
    }
    let needs_space = left
        .encode_utf16()
        .last()
        .is_some_and(|c| c <= 127 && (c as u8).is_ascii_alphanumeric())
        && right
            .encode_utf16()
            .next()
            .is_some_and(|c| c <= 127 && (c as u8).is_ascii_alphanumeric());
    if needs_space {
        format!("{left} {right}")
    } else {
        format!("{left}{right}")
    }
}

pub(super) fn merge_same_line(previous: &Line, current: &Line) -> Line {
    let left = min(previous.bounds.x, current.bounds.x);
    let top = min(previous.bounds.y, current.bounds.y);
    let right = max(previous.bounds.right(), current.bounds.right());
    let bottom = max(previous.bounds.bottom(), current.bounds.bottom());
    let text = join_inline_text(&previous.text, &current.text);
    let script = line::layout_script_for(&text);
    let blocks = [previous, current];
    Line {
        text,
        bounds: RectF::new(left, top, right - left, bottom - top),
        render_glyph_height: combine_glyph_height(
            previous.render_glyph_height,
            current.render_glyph_height,
        ),
        confidence: combine_confidence(&blocks),
        layout_script: script,
        layout_bounds: previous.layout_bounds.union(current.layout_bounds),
        layout_glyph_height: combine_layout_glyph_height(script, &blocks),
        font_glyph_height: combine_glyph_height(
            previous.font_glyph_height,
            current.font_glyph_height,
        ),
        upright_layout_bounds: combine_upright(&blocks),
        tilted_lines: combine_tilted(&blocks),
        line_geometry: combine_row_geometry(previous, current),
        ..Line::default()
    }
}

fn combine_row_geometry(previous: &Line, current: &Line) -> Option<LineGeometry> {
    let first = previous.line_geometry?;
    let second = current.line_geometry?;
    if first.angle_degrees.abs() >= LineGeometry::TILTED_TO_DEGREES
        || second.angle_degrees.abs() >= LineGeometry::TILTED_TO_DEGREES
    {
        return None;
    }
    let degrees = (first.angle_degrees * first.length + second.angle_degrees * second.length)
        / max(1e-9, first.length + second.length);
    let across = previous.layout_bounds.union(current.layout_bounds).width;
    Some(LineGeometry {
        length: across / (degrees * std::f64::consts::PI / 180.0).cos(),
        thickness: max(first.thickness, second.thickness),
        angle_degrees: degrees,
    })
}

pub fn build_group(blocks: &[Line]) -> Line {
    if blocks.len() == 1 {
        return blocks[0].clone();
    }
    let x = blocks.iter().map(|b| b.bounds.x).reduce(min).unwrap();
    let y = blocks.iter().map(|b| b.bounds.y).reduce(min).unwrap();
    let right = blocks.iter().map(|b| b.bounds.right()).reduce(max).unwrap();
    let bottom = blocks
        .iter()
        .map(|b| b.bounds.bottom())
        .reduce(max)
        .unwrap();
    let mut text = String::new();
    for part in blocks
        .iter()
        .map(|b| b.text.trim())
        .filter(|s| !s.is_empty())
    {
        // Chinese and Japanese wrap mid-word, so a break between two wide characters is no space.
        let wide_break = text.encode_utf16().last().is_some_and(line::is_wide_cjk)
            && part.encode_utf16().next().is_some_and(line::is_wide_cjk);
        if !text.is_empty() && !wide_break {
            text.push(' ');
        }
        text.push_str(part);
    }
    let script = line::layout_script_for(&text);
    let refs: Vec<_> = blocks.iter().collect();
    Line {
        text,
        bounds: RectF::new(x, y, right - x, bottom - y),
        source_line_bounds: Some(blocks.iter().map(|b| b.bounds).collect()),
        render_glyph_height: median(
            blocks
                .iter()
                .filter_map(|b| b.render_glyph_height)
                .collect(),
        ),
        confidence: combine_confidence(&refs),
        layout_script: script,
        layout_bounds: blocks
            .iter()
            .map(|b| b.layout_bounds)
            .reduce(RectF::union)
            .unwrap(),
        layout_glyph_height: combine_layout_glyph_height(script, &refs),
        font_glyph_height: median(blocks.iter().filter_map(|b| b.font_glyph_height).collect()),
        upright_layout_bounds: combine_upright(&refs),
        tilted_lines: combine_tilted(&refs),
        ..Line::default()
    }
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    (!values.is_empty()).then(|| values[values.len() / 2])
}
fn combine_confidence(blocks: &[&Line]) -> Option<f64> {
    let (mut weighted, mut weight) = (0.0, 0.0);
    for block in blocks {
        if let Some(confidence) = block.confidence {
            let characters = block.text.trim().encode_utf16().count().max(1) as f64;
            weighted += confidence * characters;
            weight += characters;
        }
    }
    (weight > 0.0).then(|| weighted / weight)
}
fn combine_layout_glyph_height(script: LayoutScript, blocks: &[&Line]) -> Option<f64> {
    if !matches!(script, LayoutScript::Latin | LayoutScript::Cjk) {
        return None;
    }
    median(
        blocks
            .iter()
            .filter_map(|b| b.layout_glyph_height.filter(|&h| h > 0.0))
            .collect(),
    )
}
fn combine_glyph_height(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some((a + b) / 2.0),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        _ => None,
    }
}
fn combine_upright(blocks: &[&Line]) -> Option<RectF> {
    blocks
        .iter()
        .any(|b| b.upright_layout_bounds.is_some())
        .then(|| {
            blocks
                .iter()
                .map(|b| b.upright_layout_bounds.unwrap_or(b.layout_bounds))
                .reduce(RectF::union)
                .unwrap()
        })
}
fn combine_tilted(blocks: &[&Line]) -> Option<Vec<TiltedLine>> {
    blocks.iter().all(|b| b.tilted_lines.is_some()).then(|| {
        blocks
            .iter()
            .flat_map(|b| b.tilted_lines.as_ref().unwrap().iter().cloned())
            .collect()
    })
}
