use super::{
    engine::{self, Context, Node},
    gap::SameLineGapThreshold,
    next_line,
    profile::GroupingProfile,
    trace::NextLineDecision,
};
use crate::screen::{
    geom::{max, min, RectF},
    line::{self, LayoutScript, TextSizeBasis},
};
use std::rc::Rc;

const SIZE_BREAK_MIN_GAP: f64 = 0.15;
const SIZE_BREAK_MAX_RATIO: f64 = 0.7;
const SIZE_BREAK_MIN_GLYPHS: usize = 4;
const SIZE_BREAK_MIN_SMALLER_CHARACTERS: usize = 3;
const MINIMUM_ROW_HEIGHT_RATIO: f64 = 0.5;
const MINIMUM_ROW_VERTICAL_OVERLAP: f64 = 0.72;
const MINIMUM_ROW_GAP_PIXELS: f64 = 6.0;
const SPANNING_ROW_MAX_ADVANCE: f64 = 1.65;
const PARAGRAPH_ROW_MAX_ADVANCE: f64 = 1.45;

pub(super) fn merge_same_line_fragments(
    blocks: &[Node],
    profile: &GroupingProfile,
    context: &mut Context,
) -> Vec<Node> {
    let rows = build_visual_rows(blocks);
    let gaps: Vec<_> = rows
        .iter()
        .flat_map(|row| {
            row.windows(2)
                .map(|pair| normalized_gap(&pair[0], &pair[1]))
        })
        .collect();
    let threshold = SameLineGapThreshold::estimate(&gaps);
    rows.iter()
        .enumerate()
        .flat_map(|(index, row)| {
            split_row_into_lines(row, index, threshold.value, blocks, profile, &rows, context)
        })
        .collect()
}

fn build_visual_rows(blocks: &[Node]) -> Vec<Vec<Node>> {
    let mut ordered = blocks.to_vec();
    ordered.sort_by(|a, b| {
        a.layout_bounds
            .x
            .total_cmp(&b.layout_bounds.x)
            .then(a.layout_bounds.y.total_cmp(&b.layout_bounds.y))
    });
    let mut rows: Vec<Vec<Node>> = Vec::new();
    for block in ordered {
        let (mut best_row, mut best_overlap) = (None, f64::NEG_INFINITY);
        for (i, row) in rows.iter().enumerate() {
            let rightmost = row.last().unwrap();
            if !shares_visual_row(rightmost, &block) {
                continue;
            }
            let overlap = vertical_overlap_rate(rightmost, &block);
            if overlap <= best_overlap {
                continue;
            }
            best_overlap = overlap;
            best_row = Some(i);
        }
        if let Some(i) = best_row {
            rows[i].push(block);
        } else {
            rows.push(vec![block]);
        }
    }
    rows
}

pub(super) fn shares_visual_row(previous: &line::Line, current: &line::Line) -> bool {
    line::height_ratio(previous.layout_bounds, current.layout_bounds) >= MINIMUM_ROW_HEIGHT_RATIO
        && vertical_overlap_rate(previous, current) >= MINIMUM_ROW_VERTICAL_OVERLAP
}

fn vertical_overlap_rate(previous: &line::Line, current: &line::Line) -> f64 {
    let overlap = max(
        0.0,
        min(
            previous.layout_bounds.bottom(),
            current.layout_bounds.bottom(),
        ) - max(previous.layout_bounds.y, current.layout_bounds.y),
    );
    overlap
        / max(
            1.0,
            min(previous.layout_bounds.height, current.layout_bounds.height),
        )
}

fn normalized_gap(previous: &line::Line, current: &line::Line) -> f64 {
    (current.layout_bounds.x - previous.layout_bounds.right())
        / max(
            1.0,
            (previous.layout_bounds.height + current.layout_bounds.height) / 2.0,
        )
}

fn split_row_into_lines(
    row: &[Node],
    row_index: usize,
    threshold: f64,
    blocks: &[Node],
    profile: &GroupingProfile,
    rows: &[Vec<Node>],
    context: &mut Context,
) -> Vec<Node> {
    let mut lines = Vec::new();
    let mut line = Rc::clone(&row[0]);
    let mut members = vec![Rc::clone(&row[0])];
    for i in 1..row.len() {
        let (mut joined, mut rule) = judge_same_line(&row[i - 1], &row[i], threshold);
        if !joined
            && rule == "horizontal gap"
            && profile.takes_prose_evidence
            && has_spanning_prose_continuation(&row[i - 1], &row[i], blocks)
        {
            joined = true;
            rule = "spanning prose continuation";
        }
        if !joined
            && rule == "horizontal gap"
            && profile.takes_prose_evidence
            && has_paragraph_row_evidence(row, row_index, i, rows, threshold)
        {
            joined = true;
            rule = "paragraph row evidence";
        }
        if context.collect {
            context.decisions.push(same_line_decision(
                &row[i - 1],
                &row[i],
                joined,
                rule,
                context,
            ));
        }
        if joined {
            line = Rc::new(engine::merge_same_line(&line, &row[i]));
            members.push(Rc::clone(&row[i]));
            continue;
        }
        if context.collect {
            context.trace.register_line(&line, &members);
        }
        lines.push(line);
        line = Rc::clone(&row[i]);
        members = vec![Rc::clone(&row[i])];
    }
    if context.collect {
        context.trace.register_line(&line, &members);
    }
    lines.push(line);
    lines
}

fn has_spanning_prose_continuation(left: &line::Line, right: &line::Line, blocks: &[Node]) -> bool {
    let height = (left.layout_bounds.height + right.layout_bounds.height) / 2.0;
    if !shares_visual_row(left, right)
        || normalized_gap(left, right) > 1.2
        || !line::spans(left.layout_bounds, line::WRAPPED_LINE_MIN_ASPECT)
        || !line::spans(right.layout_bounds, line::WRAPPED_LINE_MIN_ASPECT)
    {
        return false;
    }
    let row = left.layout_bounds.union(right.layout_bounds);
    blocks.iter().any(|next| {
        next.layout_bounds.y >= row.bottom() - height * 0.2
            && next.layout_bounds.y - row.y <= height * SPANNING_ROW_MAX_ADVANCE
            && (next.layout_bounds.x - row.x).abs() <= height * 0.35
            && (next.layout_bounds.right() - row.right()).abs() <= height * 0.75
            && line::within_size_band(next.layout_bounds.height, height)
    })
}

fn row_bounds(row: &[Node]) -> RectF {
    row.iter()
        .map(|b| b.layout_bounds)
        .reduce(RectF::union)
        .unwrap()
}

fn has_paragraph_row_evidence(
    row: &[Node],
    row_index: usize,
    right_index: usize,
    rows: &[Vec<Node>],
    threshold: f64,
) -> bool {
    let left = &row[right_index - 1];
    let right = &row[right_index];
    if normalized_gap(left, right) > 1.5 || !shares_visual_row(left, right) {
        return false;
    }
    let bounds = row_bounds(row);
    let height = bounds.height;
    if !line::spans(bounds, line::PARAGRAPH_ROW_MIN_ASPECT)
        || row.iter().any(|b| b.layout_script != LayoutScript::Latin)
    {
        return false;
    }
    let cut = (left.layout_bounds.right() + right.layout_bounds.x) / 2.0;
    let mut supporting = Vec::new();
    for (index, other) in rows.iter().enumerate() {
        if index == row_index {
            continue;
        }
        let rectangle = row_bounds(other);
        if other.iter().any(|b| b.layout_script != LayoutScript::Latin)
            || !line::within_size_band(rectangle.height, height)
            || (rectangle.y - bounds.y).abs() < height * 0.5
            || (rectangle.y - bounds.y).abs() > height * 2.5
            || (rectangle.x - bounds.x).abs() > height * 0.5
            || (rectangle.right() - bounds.right()).abs() > height * 0.75
        {
            continue;
        }
        for pair in other.windows(2) {
            let (joined, rule) = judge_same_line(&pair[0], &pair[1], threshold);
            let other_cut = (pair[0].layout_bounds.right() + pair[1].layout_bounds.x) / 2.0;
            if !joined && rule == "horizontal gap" && (other_cut - cut).abs() <= height {
                return false;
            }
        }
        supporting.push(other);
    }
    if supporting.len() < 2 || !supporting.iter().any(|row| row.len() == 1) {
        return false;
    }
    let mut tops: Vec<_> = supporting
        .iter()
        .map(|row| row_bounds(row))
        .chain(std::iter::once(bounds))
        .collect();
    tops.sort_by(|a, b| a.y.total_cmp(&b.y));
    for pair in tops.windows(2) {
        if pair[1].y - pair[0].y > min(pair[1].height, pair[0].height) * PARAGRAPH_ROW_MAX_ADVANCE {
            return false;
        }
    }
    true
}

fn judge_same_line(
    previous: &line::Line,
    current: &line::Line,
    threshold: f64,
) -> (bool, &'static str) {
    let avg_height = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
    let horizontal_gap = current.layout_bounds.x - previous.layout_bounds.right();
    if horizontal_gap < -avg_height {
        return (false, "overlaps too far");
    }
    if horizontal_gap > max(avg_height * threshold, MINIMUM_ROW_GAP_PIXELS) {
        return (false, "horizontal gap");
    }
    if is_size_break_in_row(previous, current, horizontal_gap, avg_height) {
        (false, "different size")
    } else {
        (true, "same row")
    }
}

fn is_size_break_in_row(
    previous: &line::Line,
    current: &line::Line,
    horizontal_gap: f64,
    avg_height: f64,
) -> bool {
    if horizontal_gap <= avg_height * SIZE_BREAK_MIN_GAP {
        return false;
    }
    let (ratio, basis, previous_size, current_size) =
        next_line::text_size_ratio_with_basis(previous, current);
    if ratio >= SIZE_BREAK_MAX_RATIO || basis != TextSizeBasis::Glyph {
        return false;
    }
    let previous_smaller = previous_size < current_size;
    has_steady_size(previous, previous_smaller) && has_steady_size(current, !previous_smaller)
}

fn has_steady_size(block: &line::Line, is_smaller: bool) -> bool {
    match block.layout_script {
        LayoutScript::Unknown => false,
        LayoutScript::Latin => {
            block
                .text
                .encode_utf16()
                .filter(|&c| !line::is_whitespace(c))
                .count()
                >= SIZE_BREAK_MIN_GLYPHS
        }
        _ => {
            block
                .text
                .encode_utf16()
                .filter(|&c| line::is_letter(c) || line::is_digit(c))
                .count()
                >= if is_smaller {
                    SIZE_BREAK_MIN_SMALLER_CHARACTERS
                } else {
                    1
                }
        }
    }
}

fn same_line_decision(
    previous: &Node,
    current: &Node,
    joined: bool,
    rule: &'static str,
    context: &Context,
) -> NextLineDecision {
    NextLineDecision {
        kind: "row",
        previous: previous.text.clone(),
        current: current.text.clone(),
        previous_script: previous.layout_script,
        current_script: current.layout_script,
        vertical_gap: normalized_gap(previous, current),
        left_delta: vertical_overlap_rate(previous, current),
        center_delta: 0.0,
        right_delta: 0.0,
        alignment_delta: 0.0,
        text_size_ratio: 0.0,
        width_ratio: previous.layout_bounds.width / max(1.0, current.layout_bounds.width),
        line_advance: 0.0,
        leading_bar: 0.0,
        solid_bar: 0.0,
        joined,
        rule,
        previous_id: context.trace.block_id(previous),
        current_id: context.trace.block_id(current),
        size_basis: TextSizeBasis::None,
        previous_size_value: 0.0,
        current_size_value: 0.0,
        advance_pixels: 0.0,
        advance_denominator: 0.0,
    }
}
