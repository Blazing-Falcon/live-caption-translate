use super::super::{
    geom::{max, min, RectF},
    line::{glyphs_in, layout_script_for, Line},
    ImageView,
};

pub fn for_recognition(points: [(i32, i32); 4]) -> [(i32, i32); 4] {
    let distance = |a: (i32, i32), b: (i32, i32)| {
        ((f64::from(b.0 - a.0)).powi(2) + (f64::from(b.1 - a.1)).powi(2)).sqrt()
    };
    if distance(points[0], points[3]) >= distance(points[0], points[1]) * 1.5 {
        [points[1], points[2], points[3], points[0]]
    } else {
        points
    }
}

pub fn glyph_pitch(block: &Line) -> f64 {
    let width = if block.layout_bounds.width > 0.0 {
        block.layout_bounds.width
    } else {
        block.bounds.width
    };
    let advance = block.bounds.height / glyphs_in(&block.text).max(1) as f64;
    min(
        width,
        if width >= advance * 3.0 {
            (width * advance).sqrt()
        } else {
            advance
        },
    )
}

pub fn prepare_blocks(blocks: Vec<Line>) -> Vec<Line> {
    blocks
        .into_iter()
        .filter(|b| b.bounds.width > 0.0 && b.bounds.height > 0.0)
        .map(|mut block| {
            let pitch = glyph_pitch(&block);
            block.layout_bounds = block.bounds;
            block.layout_script = layout_script_for(&block.text);
            block.layout_glyph_height = Some(pitch);
            block.render_glyph_height = Some(pitch);
            block
        })
        .collect()
}

pub fn join_column_fragments(mut blocks: Vec<Line>, pixels: Option<&ImageView<'_>>) -> Vec<Line> {
    blocks.sort_by(|a, b| {
        a.layout_bounds
            .y
            .total_cmp(&b.layout_bounds.y)
            .then_with(|| b.layout_bounds.right().total_cmp(&a.layout_bounds.right()))
    });
    let mut columns: Vec<Line> = Vec::new();
    for next in blocks {
        let chosen = columns
            .iter()
            .enumerate()
            .filter(|(_, column)| continues(column, &next) && !has_divider(column, &next, pixels))
            .min_by(|(_, a), (_, b)| {
                (next.layout_bounds.y - a.layout_bounds.bottom())
                    .total_cmp(&(next.layout_bounds.y - b.layout_bounds.bottom()))
            })
            .map(|(i, _)| i);
        if let Some(i) = chosen {
            let column = &mut columns[i];
            let n = column.text.encode_utf16().count();
            let m = next.text.encode_utf16().count();
            column.confidence = match (column.confidence, next.confidence) {
                (Some(a), Some(b)) if n + m > 0 => {
                    Some((a * n as f64 + b * m as f64) / (n + m) as f64)
                }
                _ => None,
            };
            column.text.push_str(&next.text);
            column.bounds = column.bounds.union(next.bounds);
            column.layout_bounds = column.layout_bounds.union(next.layout_bounds);
            column.layout_script = layout_script_for(&column.text);
            column.source_line_bounds = None;
        } else {
            columns.push(next);
        }
    }
    columns
}

fn continues(column: &Line, next: &Line) -> bool {
    let a = column.layout_bounds;
    let b = next.layout_bounds;
    let width = min(a.width, b.width);
    if width <= 0.0 || max(a.width, b.width) > width * 1.5 {
        return false;
    }
    let overlap = min(a.right(), b.right()) - max(a.x, b.x);
    let gap = b.y - a.bottom();
    overlap >= width * 0.7 && gap >= 0.0 && gap <= min(glyph_pitch(column), glyph_pitch(next)) * 0.4
}

fn has_divider(column: &Line, next: &Line, pixels: Option<&ImageView<'_>>) -> bool {
    let Some(pixels) = pixels else {
        return false;
    };
    let a = column.layout_bounds;
    let b = next.layout_bounds;
    let width = min(a.width, b.width);
    let left = (max(a.x, b.x) - width * 0.35).floor().max(0.0) as u32;
    let right = (min(a.right(), b.right()) + width * 0.35)
        .ceil()
        .min(f64::from(pixels.width)) as u32;
    let top = (a.bottom() - width * 0.5).floor().max(0.0) as u32;
    let bottom = (b.y + width * 0.5).ceil().min(f64::from(pixels.height)) as u32;
    let covered = (left..right)
        .filter(|&x| {
            (top..bottom).any(|y| {
                let pixel = &pixels.row(y)[x as usize * 4..][..3];
                *pixel.iter().max().unwrap() < 128
            })
        })
        .count();
    right > left && covered as f64 >= f64::from(right - left) * 0.8
}

pub fn to_upright(box_rect: RectF, source_width: u32) -> RectF {
    RectF::new(
        f64::from(source_width) - box_rect.bottom(),
        box_rect.x,
        box_rect.height,
        box_rect.width,
    )
}

pub fn block_to_upright(mut block: Line, source_width: u32) -> Line {
    block.bounds = to_upright(block.bounds, source_width);
    block.layout_bounds = to_upright(block.layout_bounds, source_width);
    block.line_geometry = block.line_geometry.map(|line| line.turned_back());
    block.source_line_bounds = None;
    block
}
