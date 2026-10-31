//! Recognition of vertical text: column detection, crops, grouping and turned frames.

pub mod columns_first;
pub mod detection;
pub mod geometry;
pub mod grouping;
pub mod probability;
pub mod repeated;
pub mod ruby;
pub mod tilted;

use super::{
    filters,
    line::{Line, RawLine},
    raster::Raster,
    ImageView,
};
use crate::engines::TextLine;

pub fn prepare_lines(lines: &[TextLine]) -> Vec<Line> {
    lines
        .iter()
        .flat_map(|line| {
            let mut facts =
                geometry::prepare_blocks(filters::convert_blocks(&[RawLine::from(line)]));
            for fact in &mut facts {
                if let Some(pitch) = line.glyph_pitch {
                    fact.layout_glyph_height = Some(pitch);
                    fact.render_glyph_height = Some(pitch);
                }
            }
            facts
        })
        .collect()
}

pub fn turn(source: &ImageView<'_>) -> Raster {
    let mut turned = Raster {
        width: source.height,
        height: source.width,
        bgra: vec![0; source.width as usize * source.height as usize * 4],
    };
    for y in 0..source.height {
        for x in 0..source.width {
            let target = ((source.width - 1 - x) * turned.width + y) as usize * 4;
            turned.bgra[target..][..4].copy_from_slice(&source.row(y)[x as usize * 4..][..4]);
        }
    }
    turned
}
