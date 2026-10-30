use super::{
    geom::{LineGeometry, RectF},
    line::{
        glyphs_in, is_han_ideograph, layout_script_for, normalize_automatic_blocks,
        normalize_blocks, LayoutScript, Line, RawLine,
    },
};
use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;

pub const MIN_RECOGNITION_CONFIDENCE: f64 = 0.6;
pub const MAX_WIDTH_PER_GLYPH: f64 = 2.0;

pub fn convert_blocks(raw: &[RawLine]) -> Vec<Line> {
    let mut blocks = Vec::with_capacity(raw.len());
    for block in raw {
        let text = block.text.trim();
        if text.is_empty() || block.quad.is_empty() {
            continue;
        }
        let confidence = if block.char_scores.is_empty() {
            1.0_f32
        } else {
            (block.char_scores.iter().map(|v| f64::from(*v)).sum::<f64>()
                / block.char_scores.len() as f64) as f32
        };
        if f64::from(confidence) < MIN_RECOGNITION_CONFIDENCE {
            continue;
        }
        let mut line = Line::new(text, RectF::enclosing(&block.quad));
        line.confidence = Some(f64::from(confidence));
        line.line_geometry = LineGeometry::from_quad(&block.quad);
        blocks.push(line);
    }
    blocks.sort_by(|a, b| {
        a.bounds
            .y
            .total_cmp(&b.bounds.y)
            .then(a.bounds.x.total_cmp(&b.bounds.x))
    });
    blocks
}

pub fn is_latin_with_diacritic(c: char) -> bool {
    matches!(c, '\u{00c0}'..='\u{024f}' | '\u{1e00}'..='\u{1eff}')
}

pub fn fold_latin_diacritics(text: &str) -> String {
    if !text.chars().any(is_latin_with_diacritic) {
        return text.to_owned();
    }
    text.chars()
        .map(|c| {
            if !is_latin_with_diacritic(c) {
                return c;
            }
            c.to_string()
                .nfd()
                .find(|base| get_general_category(*base) != GeneralCategory::NonspacingMark)
                .filter(|base| *base != '\0')
                .unwrap_or(c)
        })
        .collect()
}

pub fn fold_block_diacritics(mut blocks: Vec<Line>) -> Vec<Line> {
    for block in &mut blocks {
        block.text = fold_latin_diacritics(&block.text);
    }
    blocks
}

pub fn strip_lone_ideographs(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let mut stripped = text;
    let mut chars = stripped.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(next))
            if u16::try_from(u32::from(first)).is_ok_and(is_han_ideograph)
                && next.is_ascii_alphabetic() =>
        {
            stripped = stripped[first.len_utf8()..].trim_start()
        }
        _ => {}
    }
    let mut chars = stripped.chars().rev();
    match (chars.next(), chars.next()) {
        (Some(last), Some(previous))
            if u16::try_from(u32::from(last)).is_ok_and(is_han_ideograph)
                && previous.is_ascii_alphabetic() =>
        {
            stripped = stripped[..stripped.len() - last.len_utf8()].trim_end()
        }
        _ => {}
    }
    if matches!(
        layout_script_for(stripped),
        LayoutScript::Cjk | LayoutScript::Mixed
    ) {
        text.to_owned()
    } else {
        stripped.to_owned()
    }
}

pub fn strip_icon_ideographs(mut blocks: Vec<Line>) -> Vec<Line> {
    for block in &mut blocks {
        block.text = strip_lone_ideographs(&block.text);
    }
    blocks
}

pub fn is_too_wide_for_its_text(block: &Line) -> bool {
    let glyphs = glyphs_in(&block.text);
    if glyphs == 0 || block.layout_bounds.height <= 0.0 {
        return false;
    }
    block.layout_bounds.width / glyphs as f64 / block.layout_bounds.height >= MAX_WIDTH_PER_GLYPH
}

pub fn remove_misshapen_blocks(blocks: Vec<Line>) -> Vec<Line> {
    blocks
        .into_iter()
        .filter(|block| !is_too_wide_for_its_text(block))
        .collect()
}

pub fn apply_block_filters(
    blocks: Vec<Line>,
    use_cjk_render_metrics: bool,
    automatic: bool,
) -> Vec<Line> {
    let converted = strip_icon_ideographs(fold_block_diacritics(blocks));
    let normalized = if automatic {
        normalize_automatic_blocks(converted)
    } else {
        normalize_blocks(converted, use_cjk_render_metrics)
    };
    remove_misshapen_blocks(normalized)
}

pub fn prepare(raw: &[RawLine]) -> Vec<Line> {
    apply_block_filters(convert_blocks(raw), true, false)
}
