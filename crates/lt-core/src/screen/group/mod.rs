use super::{
    geom::{LineGeometry, RectF},
    ink,
    line::{layout_glyph_height_for, layout_script_for, Line},
    paint, placement, Block, ImageView, Rect,
};
use crate::engines::TextLine;

pub mod engine;
pub mod gap;
pub mod next_line;
pub mod profile;
mod rows;
pub mod ruby;
pub mod speaker;
pub mod trace;

pub use engine::{build_group, group, group_with_profile, join_inline_text, GroupingReport};

pub fn blocks(image: &ImageView<'_>, lines: &[TextLine]) -> Vec<Block> {
    let mut facts: Vec<_> = lines
        .iter()
        .map(|line| {
            let bounds = RectF::from(line.rect);
            let script = layout_script_for(&line.text);
            Line {
                text: line.text.clone(),
                bounds,
                layout_bounds: bounds,
                layout_script: script,
                layout_glyph_height: layout_glyph_height_for(script, bounds, &line.text),
                confidence: Some(f64::from(line.score)),
                line_geometry: line
                    .quad
                    .as_ref()
                    .and_then(|quad| LineGeometry::from_quad(quad)),
                ..Line::default()
            }
        })
        .collect();
    ink::annotate(image, &mut facts);
    blocks_from_groups(image, &group(&facts))
}

pub fn blocks_from_groups(image: &ImageView<'_>, groups: &[Line]) -> Vec<Block> {
    let lines: Vec<Vec<Rect>> = groups
        .iter()
        .map(|group| {
            group
                .lines()
                .iter()
                .map(|bounds| bounds.outward())
                .collect()
        })
        .collect();
    let every_line: Vec<_> = lines.iter().flatten().copied().collect();
    let surface = paint::Surface::new(image, &every_line);
    groups
        .iter()
        .zip(lines)
        .enumerate()
        .map(|(id, (group, lines))| {
            let background = surface.background(&lines);
            let mut heights: Vec<_> = lines.iter().map(|line| line.h).collect();
            heights.sort_unstable();
            Block {
                id: id as u32,
                rect: group.bounds.outward(),
                line_height: heights[heights.len() / 2],
                translate: group.text.chars().any(is_han),
                foreground: paint::foreground(background),
                background,
                intent: placement::intent_for(group),
                lines,
                source: group.text.clone(),
            }
        })
        .collect()
}

pub(super) fn is_han(c: char) -> bool {
    matches!(c,'\u{3007}'|'\u{3400}'..='\u{4dbf}'|'\u{4e00}'..='\u{9fff}'|'\u{f900}'..='\u{faff}'|'\u{20000}'..='\u{2fa1f}')
}
