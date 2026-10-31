//! Groups vertical columns into blocks, with ruby, rows lying on columns and writing that runs
//! across.

use super::super::{
    filters,
    geom::{max, min, RectF},
    group, ink,
    line::{glyphs_in, is_letter, layout_script_for, LayoutScript, Line},
    ImageView,
};
use super::{
    geometry::{glyph_pitch, join_column_fragments},
    repeated, ruby, tilted,
};

/// How small a group must be, against the one it sits on, to be the ruby over it. Ruby is set at
/// about half the body size.
const RUBY_MAX_RELATIVE_SIZE: f64 = 0.60;
/// How much of the smaller group must lie on the other one. A looser bar would start catching small
/// aside balloons beside a big one.
const RUBY_SHARED_AREA: f64 = 0.50;
/// How much of a row must lie on a group of columns before it is taken as a misread of them. Low,
/// because a real row never touches a column.
const ROW_LYING_ON_COLUMNS: f64 = 0.15;
/// How much of the shorter of two runs of columns must lie alongside the other for the two to be
/// one block.
const RUNS_ALONGSIDE: f64 = 0.65;
/// How much longer than thick a detector quadrilateral near level must be for its writing to be
/// taken as running across, whatever its upright box says.
pub const ACROSS_ALONG_ITSELF: f64 = 1.6;
/// How much of the shorter column must run alongside the other for the two to be one piece of
/// writing. Balloon columns start at different heights, so tops are not compared.
const SIDE_BY_SIDE_ALONG_THE_COLUMN: f64 = 0.35;
/// How far apart two columns may be, in characters of the smaller type. Big lettering would
/// otherwise stretch the distance bar to reach a balloon beside it.
const SMALLER_TYPE_REACH: f64 = 4.0;

pub fn group(blocks: Vec<Line>, pixels: Option<&ImageView<'_>>, cjk: Option<bool>) -> Vec<Line> {
    let blocks = repeated::drop_repeated(blocks);
    let (writing, readings) = ruby::separate(blocks);
    let (candidates, across): (Vec<_>, Vec<_>) = writing.into_iter().partition(|b| !runs_across(b));
    let mut merged = merge_columns(join_column_fragments(candidates.clone(), pixels));
    merged.extend(across.into_iter().map(|mut b| {
        b.runs_across = true;
        b
    }));
    let mut groups =
        without_wordless_groups(without_rows_over_columns(without_ruby(merged), &candidates));
    let aside = without_wordless_groups(
        readings
            .into_iter()
            .map(|r| combine_columns(vec![r]))
            .collect(),
    );
    let kept: Vec<_> = aside
        .into_iter()
        .filter(|r| glyphs_in(&r.text) > 1 && !groups.iter().any(|b| is_ruby_over(r, b)))
        .collect();
    groups.extend(kept);
    if let Some(cjk) = cjk {
        groups = with_rows_set_across(groups, pixels, cjk);
    }
    groups
}

fn with_rows_set_across(groups: Vec<Line>, pixels: Option<&ImageView<'_>>, cjk: bool) -> Vec<Line> {
    let Some(first) = groups.iter().position(|g| g.runs_across) else {
        return groups;
    };
    let mut rows: Vec<_> = groups
        .iter()
        .filter(|g| g.runs_across)
        .map(|r| Line {
            text: r.text.clone(),
            bounds: r.bounds,
            confidence: r.confidence,
            line_geometry: r.line_geometry,
            ..Line::default()
        })
        .collect();
    rows.sort_by(|a, b| {
        a.bounds
            .y
            .total_cmp(&b.bounds.y)
            .then_with(|| a.bounds.x.total_cmp(&b.bounds.x))
    });
    let mut lines = filters::apply_block_filters(rows, cjk, false);
    if let Some(pixels) = pixels {
        ink::annotate(pixels, &mut lines);
    }
    let set = group::group_screenshot(&lines);
    let mut result = Vec::new();
    for (i, g) in groups.into_iter().enumerate() {
        if i == first {
            result.extend(set.iter().cloned().map(|mut g| {
                g.runs_across = true;
                g
            }));
        }
        if !g.runs_across {
            result.push(g);
        }
    }
    result
}

pub fn without_wordless_groups(groups: Vec<Line>) -> Vec<Line> {
    groups
        .into_iter()
        .filter(|g| g.text.encode_utf16().any(is_letter))
        .collect()
}
fn is_ruby_over(candidate: &Line, body: &Line) -> bool {
    let (Some(reading), Some(text)) = (candidate.render_glyph_height, body.render_glyph_height)
    else {
        return false;
    };
    if candidate.runs_across || reading >= text * RUBY_MAX_RELATIVE_SIZE {
        return false;
    }
    let shared = candidate.bounds.intersect(body.bounds);
    let area = candidate.bounds.width * candidate.bounds.height;
    !shared.is_empty() && area > 0.0 && shared.width * shared.height / area >= RUBY_SHARED_AREA
}
pub fn without_ruby(groups: Vec<Line>) -> Vec<Line> {
    groups
        .iter()
        .enumerate()
        .filter(|(i, g)| {
            !groups
                .iter()
                .enumerate()
                .any(|(j, b)| i != &j && is_ruby_over(g, b))
        })
        .map(|(_, g)| g.clone())
        .collect()
}
pub fn without_rows_over_columns(groups: Vec<Line>, columns: &[Line]) -> Vec<Line> {
    if !groups.iter().any(|g| g.runs_across) || !groups.iter().any(|g| !g.runs_across) {
        return groups;
    }
    groups
        .iter()
        .filter(|g| {
            !g.runs_across
                || !(groups.iter().any(|c| {
                    !c.runs_across
                        && c.bounds.height > c.bounds.width
                        && repeated::overlap(g.bounds, c.bounds) >= ROW_LYING_ON_COLUMNS
                }) || columns.iter().any(|c| {
                    c.bounds.height > c.bounds.width && {
                        let row = g.bounds;
                        let grown = RectF::new(
                            row.x - row.height,
                            row.y - row.height,
                            row.width + 2.0 * row.height,
                            row.height * 3.0,
                        );
                        let shared = grown.intersect(c.bounds);
                        !shared.is_empty() && shared.width > 0.0 && shared.height > 0.0
                    }
                }))
        })
        .cloned()
        .collect()
}
pub fn runs_across(block: &Line) -> bool {
    const MAX_WIDTH_TO_HEIGHT_RATIO: f64 = 1.4;
    glyphs_in(&block.text) > 1
        && (block.layout_bounds.width > block.layout_bounds.height * MAX_WIDTH_TO_HEIGHT_RATIO
            || block.line_geometry.is_some_and(|l| {
                l.angle_degrees.abs() <= 45.0 && l.length >= l.thickness * ACROSS_ALONG_ITSELF
            }))
}
fn is_same_group(a: &Line, b: &Line) -> bool {
    let (pa, pb) = (glyph_pitch(a), glyph_pitch(b));
    let (a, b) = (a.layout_bounds, b.layout_bounds);
    let shared = min(a.bottom(), b.bottom()) - max(a.y, b.y);
    let shorter = min(a.height, b.height);
    if shorter <= 0.0 || shared < shorter * SIDE_BY_SIDE_ALONG_THE_COLUMN {
        return false;
    }
    let distance = ((a.x + a.right()) / 2.0 - (b.x + b.right()) / 2.0).abs();
    distance <= max(pa, pb) * 1.6 && distance <= min(pa, pb) * SMALLER_TYPE_REACH
}
pub fn merge_columns(columns: Vec<Line>) -> Vec<Line> {
    let mut remaining: Vec<_> = columns.into_iter().filter(|c| !runs_across(c)).collect();
    remaining.sort_by(|a, b| b.layout_bounds.x.total_cmp(&a.layout_bounds.x));
    let mut merged = Vec::new();
    while !remaining.is_empty() {
        let mut members = vec![remaining.remove(0)];
        loop {
            let mut grew = false;
            for i in (0..remaining.len()).rev() {
                if members.iter().any(|m| is_same_group(m, &remaining[i])) {
                    members.push(remaining.remove(i));
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        merged.extend(reading_blocks(members).into_iter().map(combine_columns));
    }
    merged
}
fn span(columns: &[Line]) -> RectF {
    columns
        .iter()
        .fold(RectF::EMPTY, |r, c| r.union(c.layout_bounds))
}
fn reading_blocks(group: Vec<Line>) -> Vec<Vec<Line>> {
    if group.len() < 2 {
        return vec![group];
    }
    let mut ordered = group.clone();
    ordered.sort_by(|a, b| {
        ((b.layout_bounds.x + b.layout_bounds.right()) / 2.0)
            .total_cmp(&((a.layout_bounds.x + a.layout_bounds.right()) / 2.0))
    });
    let (mut cut, mut least) = (0, RUNS_ALONGSIDE);
    for k in 1..ordered.len() {
        let (a, b) = (span(&ordered[..k]), span(&ordered[k..]));
        if min(a.right(), b.right()) - max(a.x, b.x) > 0.5 * min(a.width, b.width) {
            continue;
        }
        let shared = min(a.bottom(), b.bottom()) - max(a.y, b.y);
        let shorter = min(a.height, b.height);
        if shorter > 0.0 && shared < least * shorter {
            least = shared / shorter;
            cut = k;
        }
    }
    if cut == 0 {
        return vec![group];
    }
    let mut result = reading_blocks(ordered[..cut].to_vec());
    result.extend(reading_blocks(ordered[cut..].to_vec()));
    result
}
fn median(mut sizes: Vec<f64>) -> Option<f64> {
    sizes.sort_by(f64::total_cmp);
    sizes.get(sizes.len() / 2).copied()
}
pub fn combine_columns(mut group: Vec<Line>) -> Line {
    group.sort_by(|a, b| b.layout_bounds.x.total_cmp(&a.layout_bounds.x));
    let bounds = group.iter().fold(RectF::EMPTY, |b, c| b.union(c.bounds));
    let glyph_size = median(
        group
            .iter()
            .map(|c| {
                if let Some(pitch) = c.render_glyph_height.filter(|p| *p > 0.0) {
                    return min(c.bounds.width, pitch);
                }
                let count = glyphs_in(&c.text);
                if count > 0 {
                    min(
                        c.bounds.width,
                        (c.bounds.width * c.bounds.height / count as f64).sqrt(),
                    )
                } else {
                    c.bounds.width
                }
            })
            .filter(|s| *s > 0.0)
            .collect(),
    )
    .unwrap_or(1.0);
    let lines = if group.len() > 1 {
        group.iter().map(|c| c.bounds).collect()
    } else {
        let cells = (bounds.height / max(1.0, glyph_size))
            .round_ties_even()
            .max(1.0) as usize;
        (0..cells)
            .map(|i| {
                RectF::new(
                    bounds.x,
                    bounds.y + i as f64 * bounds.height / cells as f64,
                    bounds.width,
                    bounds.height / cells as f64,
                )
            })
            .collect()
    };
    let scored: Vec<_> = group.iter().filter(|c| c.confidence.is_some()).collect();
    let confidence = if scored.is_empty() {
        None
    } else {
        Some(
            scored
                .iter()
                .map(|c| c.confidence.unwrap() * c.text.encode_utf16().count().max(1) as f64)
                .sum::<f64>()
                / scored
                    .iter()
                    .map(|c| c.text.encode_utf16().count().max(1))
                    .sum::<usize>() as f64,
        )
    };
    let text: String = group.iter().map(|c| c.text.as_str()).collect();
    let script = layout_script_for(&text);
    let layout_glyph_height = if matches!(script, LayoutScript::Latin | LayoutScript::Cjk) {
        median(
            group
                .iter()
                .filter_map(|c| c.layout_glyph_height.filter(|s| *s > 0.0))
                .collect(),
        )
    } else {
        None
    };
    Line {
        text,
        bounds,
        source_line_bounds: Some(lines),
        render_glyph_height: Some(glyph_size),
        confidence,
        layout_script: script,
        layout_bounds: span(&group),
        layout_glyph_height,
        tilt: tilted::for_columns(&group),
        ..Line::default()
    }
}
