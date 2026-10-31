//! Text on a captured screen, in plain pixel buffers and rectangles.

pub mod contour;
pub mod detection;
pub mod erase;
pub mod filters;
pub mod geom;
pub mod group;
pub mod imaging;
pub mod ink;
mod job;
pub mod line;
pub mod paint;
pub mod placement;
pub mod raster;
pub mod repair;
pub mod tilt;
pub mod vertical;

pub use group::blocks;
pub use job::{
    piece_translator, pieces, run_job, run_job_with_output, JobOutput, TranslationCache,
    PIECE_CHARS,
};

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Horizontal,
    Vertical,
}

/// A rectangle in pixels, origin top-left.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x.saturating_add_unsigned(self.w)
    }

    pub fn bottom(&self) -> i32 {
        self.y.saturating_add_unsigned(self.h)
    }
}

/// A borrowed BGRA image, rows top to bottom, `stride` bytes per row.
#[derive(Clone, Copy, Debug)]
pub struct ImageView<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bgra: &'a [u8],
}

impl<'a> ImageView<'a> {
    pub fn new(width: u32, height: u32, stride: u32, bgra: &'a [u8]) -> Result<Self> {
        let row = u64::from(width) * 4;
        let needed = match height {
            0 => 0,
            _ => u64::from(stride) * u64::from(height - 1) + row,
        };
        if u64::from(stride) < row || (bgra.len() as u64) < needed {
            return Err(Error::Engine(format!(
                "Image buffer of {} bytes is too small for {width}x{height} with stride {stride}",
                bgra.len()
            )));
        }
        Ok(Self {
            width,
            height,
            stride,
            bgra,
        })
    }

    /// Row `y`, `width * 4` bytes.
    pub fn row(&self, y: u32) -> &'a [u8] {
        let start = y as usize * self.stride as usize;
        &self.bgra[start..start + self.width as usize * 4]
    }
}

/// A paragraph or label: the text lines that read as one unit, and how to cover them.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Block {
    /// Reading order, from 0.
    pub id: u32,
    /// The union of `lines`.
    pub rect: Rect,
    pub lines: Vec<Rect>,
    #[serde(default)]
    pub intent: placement::LayoutIntent,
    /// Median height of `lines`.
    pub line_height: u32,
    pub source: String,
    /// RGB of the surface behind the text.
    pub background: [u8; 3],
    /// RGB of text that reads on `background`.
    pub foreground: [u8; 3],
    /// False when `source` has no Han character: there is nothing to translate.
    pub translate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paint: Option<PaintFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt: Option<tilt::TiltedText>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub runs_across: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PaintFacts {
    pub bounds: geom::RectF,
    pub source_lines: Vec<geom::RectF>,
    pub render_glyph_height: Option<f64>,
    pub font_glyph_height: Option<f64>,
}

/// What a screen job reports, in this order: `Reading`, `Blocks`, one `Translated` or `Failed`
/// for each block with `translate` set in id order, `Done`. `Error` replaces everything after
/// `Reading` when the text cannot be read.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScreenEvent {
    Reading {
        job: u64,
    },
    Blocks {
        job: u64,
        blocks: Vec<Block>,
        ocr_ms: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ocr_lines: Option<Vec<Rect>>,
    },
    Translated {
        job: u64,
        id: u32,
        text: String,
    },
    Failed {
        job: u64,
        id: u32,
        message: String,
    },
    Done {
        job: u64,
        total_ms: u32,
    },
    Error {
        job: u64,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    fn single_line(id: u32, rect: Rect, source: &str, dark: bool, translate: bool) -> Block {
        Block {
            id,
            rect,
            lines: vec![rect],
            intent: placement::LayoutIntent::Single,
            line_height: rect.h,
            source: source.into(),
            background: if dark { [32, 34, 40] } else { [250, 248, 243] },
            foreground: if dark { [245, 245, 245] } else { [24, 24, 24] },
            translate,
            runs_across: id == 2,
            tilt: (id == 2).then(|| tilt::TiltedText {
                degrees: 8.0,
                sheared: false,
                centre: geom::PointF::new(128.0, 181.0),
                box_rect: geom::RectF::new(88.0, 176.0, 80.0, 10.0),
                line_quads: vec![vec![
                    geom::PointF::new(89.0, 171.0),
                    geom::PointF::new(168.0, 182.0),
                    geom::PointF::new(167.0, 192.0),
                    geom::PointF::new(88.0, 181.0),
                ]],
                column: false,
            }),
            paint: Some(PaintFacts {
                bounds: rect.into(),
                source_lines: vec![rect.into()],
                render_glyph_height: None,
                font_glyph_height: None,
            }),
        }
    }

    #[test]
    fn every_event_matches_the_shared_wire_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../app/ui/src/lib/__fixtures__/screen-events.json"
        ))
        .unwrap();
        let paragraph = Block {
            id: 0,
            rect: rect(80, 44, 640, 56),
            lines: vec![rect(80, 44, 640, 24), rect(80, 76, 400, 24)],
            intent: placement::LayoutIntent::GroupReflow,
            line_height: 24,
            source: "清晨的河边总是很安静。薄雾贴着水面慢慢散开，岸上的老人在练太极。".into(),
            background: [250, 248, 243],
            foreground: [24, 24, 24],
            translate: true,
            runs_across: false,
            tilt: None,
            paint: Some(PaintFacts {
                bounds: rect(80, 44, 640, 56).into(),
                source_lines: vec![rect(80, 44, 640, 24).into(), rect(80, 76, 400, 24).into()],
                render_glyph_height: None,
                font_glyph_height: None,
            }),
        };
        let events = vec![
            ScreenEvent::Reading { job: 3 },
            ScreenEvent::Blocks {
                job: 3,
                ocr_ms: 640,
                ocr_lines: Some(vec![rect(80, 44, 640, 24), rect(80, 76, 400, 24), rect(80, 126, 220, 22), rect(80, 170, 96, 22)]),
                blocks: vec![
                    paragraph,
                    single_line(1, rect(80, 126, 220, 22), "Thank you for your patience.", false, false),
                    single_line(2, rect(80, 170, 96, 22), "关闭窗口", true, true),
                ],
            },
            ScreenEvent::Translated {
                job: 3,
                id: 0,
                text: "The riverside is always quiet in the early morning. Thin mist spreads slowly over the water, and old people on the bank practice tai chi.".into(),
            },
            ScreenEvent::Failed {
                job: 3,
                id: 2,
                message: "Translation took too long".into(),
            },
            ScreenEvent::Done { job: 3, total_ms: 4420 },
            ScreenEvent::Error {
                job: 4,
                message: "Cannot open the OCR model: file not found".into(),
            },
        ];
        assert_eq!(serde_json::to_value(&events).unwrap(), fixture);
        assert_eq!(
            serde_json::from_value::<Vec<ScreenEvent>>(fixture).unwrap(),
            events
        );
    }
}
