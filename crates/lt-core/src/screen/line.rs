//! Line facts the grouper reads: script, glyph height estimates, text size basis and writing
//! comparisons.

use super::geom::{max, min, LineGeometry, PointF, RectF};
use serde::{Deserialize, Serialize};
use unicode_general_category::{get_general_category, GeneralCategory};

/// Glyph count from which the character pitch can be used to correct the height estimate.
pub const PITCH_CORRECTED_FROM_GLYPHS: usize = 4;
/// Fraction of a detection box that its glyphs occupy. Low rather than high: too big puts oversized
/// English over the picture, too small only makes it a little small.
pub const GLYPHS_TO_BOX_HEIGHT: f64 = 0.5;
/// Share of the box height taken as glyph height before any pitch correction.
pub const VERTICAL_SCALE: f64 = 0.82;
/// Glyph height as a multiple of the character pitch for CJK text, whose characters are square.
pub const CJK_GLYPH_HEIGHT_FROM_PITCH: f64 = 1.18;
pub const LATIN_GLYPH_HEIGHT_FROM_PITCH: f64 = 1.3;
/// A shared midpoint, so two lines read slightly differently do not render at two font scales.
pub const AUTOMATIC_GLYPH_HEIGHT_FROM_PITCH: f64 = 1.24;
/// The pitch correction applies only to boxes clearly longer than they are tall.
pub const PITCH_MIN_ASPECT: f64 = 2.0;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum LayoutScript {
    #[default]
    Unknown,
    Latin,
    Cjk,
    Mixed,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Line {
    pub text: String,
    pub bounds: RectF,
    #[serde(alias = "sourceLineBounds")]
    pub source_line_bounds: Option<Vec<RectF>>,
    #[serde(alias = "renderGlyphHeight")]
    pub render_glyph_height: Option<f64>,
    pub confidence: Option<f64>,
    #[serde(alias = "layoutScript")]
    pub layout_script: LayoutScript,
    #[serde(alias = "layoutBounds")]
    pub layout_bounds: RectF,
    #[serde(alias = "layoutGlyphHeight")]
    pub layout_glyph_height: Option<f64>,
    #[serde(alias = "layoutInkHeight")]
    pub layout_ink_height: Option<f64>,
    #[serde(alias = "fontGlyphHeight")]
    pub font_glyph_height: Option<f64>,
    #[serde(alias = "lineGeometry")]
    pub line_geometry: Option<LineGeometry>,
    #[serde(alias = "uprightLayoutBounds")]
    pub upright_layout_bounds: Option<RectF>,
    #[serde(default, alias = "tiltedLines")]
    pub tilted_lines: Option<Vec<TiltedLine>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt: Option<super::tilt::TiltedText>,
    #[serde(default, alias = "runsAcross")]
    pub runs_across: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TiltedLine {
    pub cluster: i32,
    pub sheared: bool,
    pub degrees: f64,
    pub weight: f64,
    pub centre: PointF,
    pub level: SizeF,
    pub quad: Vec<PointF>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct SizeF {
    pub width: f64,
    pub height: f64,
}

impl Line {
    pub fn new(text: impl Into<String>, bounds: RectF) -> Self {
        Self {
            text: text.into(),
            bounds,
            ..Self::default()
        }
    }

    pub fn lines(&self) -> &[RectF] {
        self.source_line_bounds
            .as_deref()
            .unwrap_or(std::slice::from_ref(&self.bounds))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RawLine {
    pub text: String,
    pub quad: Vec<PointF>,
    #[serde(alias = "charScores")]
    pub char_scores: Vec<f32>,
    #[serde(default, alias = "detectorScore")]
    pub detector_score: Option<f32>,
}

impl From<&crate::engines::TextLine> for RawLine {
    fn from(line: &crate::engines::TextLine) -> Self {
        Self {
            text: line.text.clone(),
            quad: line
                .quad
                .unwrap_or_else(|| RectF::from(line.rect).corners())
                .to_vec(),
            char_scores: if line.char_scores.is_empty() {
                vec![line.score]
            } else {
                line.char_scores.clone()
            },
            detector_score: None,
        }
    }
}

pub fn is_han_ideograph(c: u16) -> bool {
    matches!(c, 0x4e00..=0x9fff | 0x3400..=0x4dbf | 0xf900..=0xfaff)
}

pub fn is_kana(c: u16) -> bool {
    matches!(c, 0x3041..=0x30ff | 0x31f0..=0x31ff)
}

pub fn is_letter(c: u16) -> bool {
    char::from_u32(u32::from(c)).is_some_and(|c| {
        matches!(
            get_general_category(c),
            GeneralCategory::UppercaseLetter
                | GeneralCategory::LowercaseLetter
                | GeneralCategory::TitlecaseLetter
                | GeneralCategory::ModifierLetter
                | GeneralCategory::OtherLetter
        )
    })
}

pub fn is_digit(c: u16) -> bool {
    char::from_u32(u32::from(c))
        .is_some_and(|c| get_general_category(c) == GeneralCategory::DecimalNumber)
}

pub fn is_whitespace(c: u16) -> bool {
    char::from_u32(u32::from(c)).is_some_and(char::is_whitespace)
}

pub fn glyphs_in(text: &str) -> usize {
    text.encode_utf16().filter(|c| !is_whitespace(*c)).count()
}

pub fn layout_script_for(text: &str) -> LayoutScript {
    let (mut cjk, mut latin, mut readable) = (false, false, false);
    for c in text.encode_utf16() {
        if is_kana(c) || is_han_ideograph(c) {
            cjk = true;
            readable = true;
        } else if is_letter(c) {
            latin = true;
            readable = true;
        } else if is_digit(c) {
            readable = true;
        }
    }
    match (cjk, latin, readable) {
        (true, true, _) => LayoutScript::Mixed,
        (true, false, _) => LayoutScript::Cjk,
        (false, _, true) => LayoutScript::Latin,
        _ => LayoutScript::Unknown,
    }
}

pub fn pitch_clamp_applies(box_width: f64, box_height: f64, glyph_count: usize) -> bool {
    glyph_count >= PITCH_CORRECTED_FROM_GLYPHS && box_width > box_height * PITCH_MIN_ASPECT
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Correction {
    pub applied: bool,
    pub candidate: Option<f64>,
    pub selected: bool,
}

pub fn short_text_glyph_height(
    estimated: f64,
    box_width: f64,
    box_height: f64,
    glyph_count: usize,
) -> (f64, Correction) {
    if pitch_clamp_applies(box_width, box_height, glyph_count) || box_height <= 0.0 {
        return (
            estimated,
            Correction {
                applied: false,
                candidate: None,
                selected: false,
            },
        );
    }
    let candidate = box_height * GLYPHS_TO_BOX_HEIGHT;
    let corrected = min(estimated, candidate);
    (
        corrected,
        Correction {
            applied: true,
            candidate: Some(candidate),
            selected: corrected < estimated,
        },
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum GlyphHeightSource {
    None,
    Box,
    Pitch,
    ShortText,
    Floor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphHeightTrace {
    pub script: LayoutScript,
    pub box_width: f64,
    pub box_height: f64,
    pub glyph_count: usize,
    pub pitch_coefficient: f64,
    pub box_estimate: f64,
    pub pitch_candidate: Option<f64>,
    pub width_minus_twice_height: f64,
    pub has_enough_glyphs: bool,
    pub is_wide_enough: bool,
    pub pitch_branch_entered: bool,
    pub pitch_selected: bool,
    pub floor_applied: bool,
    pub short_text_applied: bool,
    pub short_text_candidate: Option<f64>,
    pub short_text_selected: bool,
    pub source: GlyphHeightSource,
    pub result: Option<f64>,
}

impl GlyphHeightTrace {
    fn not_estimated(script: LayoutScript, bounds: RectF, glyph_count: usize) -> Self {
        Self {
            script,
            box_width: bounds.width,
            box_height: bounds.height,
            glyph_count,
            pitch_coefficient: 0.0,
            box_estimate: 0.0,
            pitch_candidate: None,
            width_minus_twice_height: bounds.width - bounds.height * PITCH_MIN_ASPECT,
            has_enough_glyphs: glyph_count >= PITCH_CORRECTED_FROM_GLYPHS,
            is_wide_enough: bounds.width > bounds.height * PITCH_MIN_ASPECT,
            pitch_branch_entered: false,
            pitch_selected: false,
            floor_applied: false,
            short_text_applied: false,
            short_text_candidate: None,
            short_text_selected: false,
            source: GlyphHeightSource::None,
            result: None,
        }
    }
}

pub fn estimate_glyph_height(
    bounds: RectF,
    glyph_count: usize,
    coefficient: f64,
) -> (f64, GlyphHeightTrace) {
    let mut height = bounds.height * VERTICAL_SCALE;
    let box_estimate = height;
    let enough = glyph_count >= PITCH_CORRECTED_FROM_GLYPHS;
    let wide = bounds.width > bounds.height * PITCH_MIN_ASPECT;
    if pitch_clamp_applies(bounds.width, bounds.height, glyph_count) {
        let pitch = bounds.width / glyph_count as f64;
        height = min(height, pitch * coefficient);
    }
    let estimated = max(1.0, height);
    let mut trace = GlyphHeightTrace::not_estimated(LayoutScript::Unknown, bounds, glyph_count);
    trace.pitch_coefficient = coefficient;
    trace.box_estimate = box_estimate;
    trace.pitch_candidate =
        (glyph_count > 0).then(|| bounds.width / glyph_count as f64 * coefficient);
    trace.pitch_branch_entered = enough && wide;
    trace.pitch_selected = height < box_estimate;
    trace.floor_applied = estimated != height;
    trace.source = if estimated != height {
        GlyphHeightSource::Floor
    } else if height < box_estimate {
        GlyphHeightSource::Pitch
    } else {
        GlyphHeightSource::Box
    };
    trace.result = Some(estimated);
    (estimated, trace)
}

pub fn layout_glyph_height_for(script: LayoutScript, bounds: RectF, text: &str) -> Option<f64> {
    layout_glyph_height_traced(script, bounds, text).0
}

pub fn layout_glyph_height_traced(
    script: LayoutScript,
    bounds: RectF,
    text: &str,
) -> (Option<f64>, GlyphHeightTrace) {
    let count = glyphs_in(text);
    if !matches!(script, LayoutScript::Latin | LayoutScript::Cjk) {
        return (None, GlyphHeightTrace::not_estimated(script, bounds, count));
    }
    let coefficient = if script == LayoutScript::Cjk {
        CJK_GLYPH_HEIGHT_FROM_PITCH
    } else {
        LATIN_GLYPH_HEIGHT_FROM_PITCH
    };
    let (height, mut trace) = estimate_glyph_height(bounds, count, coefficient);
    trace.script = script;
    if script == LayoutScript::Cjk {
        return (Some(height), trace);
    }
    let (corrected, correction) =
        short_text_glyph_height(height, bounds.width, bounds.height, count);
    trace.short_text_applied = correction.applied;
    trace.short_text_candidate = correction.candidate;
    trace.short_text_selected = correction.selected;
    if correction.selected {
        trace.source = GlyphHeightSource::ShortText;
    }
    trace.result = Some(corrected);
    (Some(corrected), trace)
}

pub fn normalize_block(
    mut block: Line,
    use_cjk_render_metrics: bool,
    pitch_override: Option<f64>,
) -> Line {
    let coefficient = pitch_override.unwrap_or(if use_cjk_render_metrics {
        CJK_GLYPH_HEIGHT_FROM_PITCH
    } else {
        LATIN_GLYPH_HEIGHT_FROM_PITCH
    });
    let script = layout_script_for(&block.text);
    block.layout_script = script;
    block.layout_bounds = block.bounds;
    block.layout_glyph_height = layout_glyph_height_for(script, block.bounds, &block.text);
    let bounds = block.bounds;
    let count = glyphs_in(&block.text);
    let height = estimate_glyph_height(bounds, count, coefficient).0;
    if use_cjk_render_metrics {
        block.bounds = RectF::new(
            bounds.x,
            bounds.y + (bounds.height - height) / 2.0,
            bounds.width,
            height,
        );
        return block;
    }
    let tilted = block.line_geometry.is_some_and(LineGeometry::is_tilted);
    let render_height = short_text_glyph_height(
        height,
        if tilted { f64::INFINITY } else { bounds.width },
        bounds.height,
        count,
    )
    .0;
    let mut font_height = render_height;
    if let Some(line) = block.line_geometry.filter(|_| tilted) {
        let along = RectF::new(0.0, 0.0, line.length, line.thickness);
        font_height = short_text_glyph_height(
            estimate_glyph_height(along, count, coefficient).0,
            line.length,
            line.thickness,
            count,
        )
        .0;
    }
    block.render_glyph_height = Some(render_height);
    block.font_glyph_height = Some(font_height);
    block
}

pub fn normalize_blocks(blocks: Vec<Line>, use_cjk_render_metrics: bool) -> Vec<Line> {
    blocks
        .into_iter()
        .map(|block| normalize_block(block, use_cjk_render_metrics, None))
        .collect()
}

pub fn uses_cjk_render_metrics_for_text(text: &str) -> bool {
    text.encode_utf16().any(is_kana)
        || text.encode_utf16().filter(|c| is_han_ideograph(*c)).count() >= 2
}

pub fn normalize_automatic_blocks(blocks: Vec<Line>) -> Vec<Line> {
    blocks
        .into_iter()
        .map(|block| {
            let cjk = uses_cjk_render_metrics_for_text(&block.text);
            normalize_block(block, cjk, Some(AUTOMATIC_GLYPH_HEIGHT_FROM_PITCH))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TextSizeBasis {
    None,
    Glyph,
    BoxDifferentScript,
    BoxNoGlyphHeight,
}

/// A line this long, in line heights, may have run out of room and wrapped. Line heights serve CJK
/// and Latin text with one number.
pub const WRAPPED_LINE_MIN_ASPECT: f64 = 8.0;
/// A row of prose long enough to get the benefit of the doubt on alignment and height; a short name
/// or a stray symbol does not.
pub const SUBSTANTIAL_LINE_MIN_ASPECT: f64 = 6.0;
/// A row long enough to be a line of a paragraph rather than of an interface.
pub const PARAGRAPH_ROW_MIN_ASPECT: f64 = 12.0;
/// A body line long enough to be running text set across a column.
pub const PROSE_LINE_MIN_ASPECT: f64 = 20.0;
/// The narrowest and widest a row may be, against another, to count as the same size of text.
pub const SIZE_BAND_LOW: f64 = 0.75;
pub const SIZE_BAND_HIGH: f64 = 1.35;

pub fn aspect(bounds: RectF) -> f64 {
    if bounds.height > 0.0 {
        bounds.width / bounds.height
    } else {
        0.0
    }
}
pub fn spans(bounds: RectF, heights: f64) -> bool {
    bounds.width >= bounds.height * heights
}
pub fn height_ratio(a: RectF, b: RectF) -> f64 {
    min(a.height, b.height) / max(a.height, b.height)
}
pub fn heights_within(a: RectF, b: RectF, ratio: f64) -> bool {
    min(a.height, b.height) >= max(a.height, b.height) * ratio
}
pub fn within_size_band(height: f64, reference: f64) -> bool {
    height >= reference * SIZE_BAND_LOW && height <= reference * SIZE_BAND_HIGH
}

pub fn squeeze(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

pub fn says_the_same(one: &str, other: &str) -> bool {
    /// How much of the shorter text the two must share before they are one piece of writing.
    const ENOUGH_OF_IT: f64 = 0.6;
    let a: Vec<u16> = squeeze(one).encode_utf16().collect();
    let b: Vec<u16> = squeeze(other).encode_utf16().collect();
    let shorter = a.len().min(b.len());
    if shorter == 0 {
        return false;
    }
    let mut previous = vec![0; b.len() + 1];
    let mut current = vec![0; b.len() + 1];
    for left in a {
        for (j, right) in b.iter().enumerate() {
            current[j + 1] = if left == *right {
                previous[j] + 1
            } else {
                current[j].max(previous[j + 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()] as f64 >= shorter as f64 * ENOUGH_OF_IT
}
