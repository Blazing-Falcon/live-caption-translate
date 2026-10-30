use super::{
    engine::{Context, Node},
    profile::{GroupingProfile, MIN_TEXT_SIZE_RATIO, SOLID_LINE_ADVANCE},
    rows, speaker,
    trace::NextLineDecision,
};
use crate::screen::{
    geom::{max, min, LineGeometry},
    line::{self, LayoutScript, Line, TextSizeBasis},
};
use std::rc::Rc;
use unicode_general_category::{get_general_category, GeneralCategory};

const ALIGNMENT_TIE_BAND: f64 = 0.01;
const WRAPPED_FINAL_LINE_ADVANCE: f64 = 1.38;
const SET_SOLID_MAX_ALIGNMENT: f64 = 0.35;
const SET_SOLID_MAX_CENTRE_ALIGNMENT: f64 = 0.25;
const SET_SOLID_MIN_CENTRE_PIXELS: f64 = 5.0;
const SET_SOLID_MIN_CENTRING_INSET: f64 = 0.20;
const SET_SOLID_MAX_CENTRING_OFFSET: f64 = 0.20;
const MAX_LINE_OVERLAP: f64 = 0.5;
const MAX_LINE_GAP: f64 = 0.8;
const MAX_LINE_GAP_FLOOR_PIXELS: f64 = 10.0;
const LOOSE_PROSE_MAX_ADVANCE: f64 = 1.65;
const COLUMN_LEADING_SAMPLE_MIN: f64 = 0.8;
const COLUMN_LEADING_SAMPLE_MAX: f64 = 1.8;
const MIN_COLUMN_LEADING_SAMPLES: usize = 3;
const COLUMN_LEADING_SLACK: f64 = 1.2;
const VISIBLE_HEADING_INK_RATIO: f64 = 0.85;
const PARAGRAPH_FINAL_HEIGHT_RATIO: f64 = 0.85;

pub(super) fn group_this_line_continues(
    groups: &[Vec<Node>],
    current: &Node,
    lines: &[Node],
    profile: &GroupingProfile,
    context: &mut Context,
) -> Option<usize> {
    let (mut best, mut best_alignment, mut best_distance) = (None, f64::INFINITY, f64::INFINITY);
    for i in (0..groups.len()).rev() {
        let group = &groups[i];
        let previous = group.last().unwrap();
        if i != groups.len() - 1 && !is_within_continuation_reach(previous, current) {
            continue;
        }
        if !nothing_lies_between(previous, current, lines) {
            continue;
        }
        if !can_join_next_line(group, current, profile, lines, context) {
            continue;
        }
        let avg_height = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
        let alignment = alignment_delta(previous, current) / max(1.0, avg_height);
        let distance = (current.layout_bounds.y - previous.layout_bounds.y) / max(1.0, avg_height);
        if best.is_none()
            || alignment < best_alignment - ALIGNMENT_TIE_BAND
            || ((alignment - best_alignment).abs() <= ALIGNMENT_TIE_BAND
                && distance < best_distance)
        {
            best = Some(i);
            best_alignment = alignment;
            best_distance = distance;
        }
    }
    best
}

fn is_within_continuation_reach(previous: &Line, current: &Line) -> bool {
    let avg_height = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
    let vertical_gap = current.layout_bounds.y - previous.layout_bounds.bottom();
    vertical_gap >= -avg_height * MAX_LINE_OVERLAP
        && vertical_gap <= max(avg_height * MAX_LINE_GAP, MAX_LINE_GAP_FLOOR_PIXELS)
}

fn nothing_lies_between(previous: &Node, current: &Node, lines: &[Node]) -> bool {
    let left = max(previous.layout_bounds.x, current.layout_bounds.x);
    let right = min(
        previous.layout_bounds.right(),
        current.layout_bounds.right(),
    );
    if right <= left {
        return true;
    }
    for line in lines {
        if Rc::ptr_eq(line, previous) || Rc::ptr_eq(line, current) {
            continue;
        }
        let rectangle = line.layout_bounds;
        if rectangle.right() <= left || rectangle.x >= right {
            continue;
        }
        let x = (rectangle.x + rectangle.width / 2.0).clamp(left, right);
        let middle = rectangle.y + rectangle.height / 2.0;
        if middle > band(previous, x).1 && middle < band(current, x).0 {
            return false;
        }
    }
    true
}

fn band(line: &Line, x: f64) -> (f64, f64) {
    let rectangle = line.layout_bounds;
    let Some(geometry) = line.line_geometry else {
        return (rectangle.y, rectangle.bottom());
    };
    if line.upright_layout_bounds.is_some()
        || geometry.angle_degrees.abs() >= LineGeometry::TILTED_TO_DEGREES
    {
        return (rectangle.y, rectangle.bottom());
    }
    let middle = rectangle.y
        + rectangle.height / 2.0
        + (x - (rectangle.x + rectangle.width / 2.0))
            * (geometry.angle_degrees * std::f64::consts::PI / 180.0).tan();
    (
        max(rectangle.y, middle - geometry.thickness / 2.0),
        min(rectangle.bottom(), middle + geometry.thickness / 2.0),
    )
}

fn has_unresolved_row_fragment(previous: &Node, current: &Node, lines: &[Node]) -> bool {
    lines.iter().any(|other| {
        !Rc::ptr_eq(other, previous)
            && !Rc::ptr_eq(other, current)
            && (conflicts(previous, current, other) || conflicts(current, previous, other))
    })
}

fn conflicts(row_part: &Line, continuation: &Line, other: &Line) -> bool {
    if !rows::shares_visual_row(row_part, other) {
        return false;
    }
    let overlap = min(
        continuation.layout_bounds.right(),
        other.layout_bounds.right(),
    ) - max(continuation.layout_bounds.x, other.layout_bounds.x);
    overlap
        > min(
            continuation.layout_bounds.height,
            other.layout_bounds.height,
        ) * 0.2
}

fn can_join_next_line(
    group: &[Node],
    current: &Node,
    profile: &GroupingProfile,
    lines: &[Node],
    context: &mut Context,
) -> bool {
    let previous = group.last().unwrap();
    let paragraph_final = is_established_paragraph_final_line(group, current, profile);
    let loose_prose = is_loosely_set_prose(previous, current, profile, lines);
    let (mut joined, mut rule) =
        judge_next_line(previous, current, profile, paragraph_final, loose_prose);
    if joined
        && loose_prose
        && line_advance_ratio(previous, current) > solid_bar_for(previous, profile)
    {
        rule = "regular prose leading";
    }
    if joined
        && paragraph_final
        && text_size_ratio(previous, current)
            < min(MIN_TEXT_SIZE_RATIO, profile.tightly_set_min_text_size_ratio)
    {
        rule = "paragraph final line";
    }
    if joined && has_unresolved_row_fragment(previous, current, lines) {
        joined = false;
        rule = "unresolved row fragment";
    }
    if !context.collect {
        return joined;
    }
    let avg_height = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
    let (size_ratio, basis, previous_size, current_size) =
        text_size_ratio_with_basis(previous, current);
    let advance_pixels = current.layout_bounds.y - previous.layout_bounds.y;
    let advance_denominator = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
    context.decisions.push(NextLineDecision {
        kind: "next",
        previous: previous.text.clone(),
        current: current.text.clone(),
        previous_script: previous.layout_script,
        current_script: current.layout_script,
        vertical_gap: (current.layout_bounds.y - previous.layout_bounds.bottom())
            / max(1.0, avg_height),
        left_delta: (previous.layout_bounds.x - current.layout_bounds.x).abs()
            / max(1.0, avg_height),
        center_delta: (center_x(previous) - center_x(current)).abs() / max(1.0, avg_height),
        right_delta: (previous.layout_bounds.right() - current.layout_bounds.right()).abs()
            / max(1.0, avg_height),
        alignment_delta: alignment_delta(previous, current) / max(1.0, avg_height),
        text_size_ratio: size_ratio,
        width_ratio: previous.layout_bounds.width / max(1.0, current.layout_bounds.width),
        line_advance: line_advance_ratio(previous, current),
        leading_bar: WRAPPED_FINAL_LINE_ADVANCE,
        solid_bar: solid_bar_for(previous, profile),
        joined,
        rule,
        previous_id: context.trace.line_id(previous),
        current_id: context.trace.line_id(current),
        size_basis: basis,
        previous_size_value: previous_size,
        current_size_value: current_size,
        advance_pixels,
        advance_denominator,
    });
    joined
}

fn line_advance_ratio(previous: &Line, current: &Line) -> f64 {
    let rectangle = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
    if rectangle > 0.0 {
        (current.layout_bounds.y - previous.layout_bounds.y) / rectangle
    } else {
        -1.0
    }
}

fn center_x(line: &Line) -> f64 {
    line.layout_bounds.x + line.layout_bounds.width / 2.0
}
fn alignment_delta(previous: &Line, current: &Line) -> f64 {
    min(
        (previous.layout_bounds.x - current.layout_bounds.x).abs(),
        min(
            (previous.layout_bounds.right() - current.layout_bounds.right()).abs(),
            (center_x(previous) - center_x(current)).abs(),
        ),
    )
}

fn judge_next_line(
    previous: &Line,
    current: &Line,
    profile: &GroupingProfile,
    paragraph_final: bool,
    loose_prose: bool,
) -> (bool, &'static str) {
    let avg_height = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
    let ratio = text_size_ratio(previous, current);
    if profile.takes_prose_evidence
        && previous.layout_script != current.layout_script
        && previous.layout_bounds.width < current.layout_bounds.width
    {
        if let (Some(a), Some(b)) = (previous.layout_ink_height, current.layout_ink_height) {
            if a > 0.0 && b > 0.0 && b < a * VISIBLE_HEADING_INK_RATIO {
                return (false, "visible heading size");
            }
        }
    }
    if ratio < min(MIN_TEXT_SIZE_RATIO, profile.tightly_set_min_text_size_ratio) && !paragraph_final
    {
        return (false, "text size");
    }
    let vertical_gap =
        current.layout_bounds.y - (previous.layout_bounds.y + previous.layout_bounds.height);
    if vertical_gap < -avg_height * MAX_LINE_OVERLAP
        || vertical_gap > max(avg_height * MAX_LINE_GAP, MAX_LINE_GAP_FLOOR_PIXELS)
    {
        return (false, "vertical gap");
    }
    let japanese_prose = is_japanese_prose_continuation(previous, current, profile);
    let alignment = alignment_delta(previous, current);
    if alignment > max(avg_height * 1.2, 18.0) && !japanese_prose {
        return (false, "alignment");
    }
    let overlap = max(
        0.0,
        min(
            previous.layout_bounds.right(),
            current.layout_bounds.right(),
        ) - max(previous.layout_bounds.x, current.layout_bounds.x),
    );
    let overlap_rate = overlap
        / max(
            1.0,
            min(previous.layout_bounds.width, current.layout_bounds.width),
        );
    let aligned = overlap_rate >= 0.35 || alignment <= max(avg_height * 0.7, 12.0);
    if !aligned {
        return (false, "not aligned enough to continue");
    }
    sentence_continuation_evidence(
        previous,
        current,
        profile,
        ratio,
        loose_prose,
        japanese_prose,
    )
}

pub fn text_size_ratio(previous: &Line, current: &Line) -> f64 {
    text_size_ratio_with_basis(previous, current).0
}

pub fn text_size_ratio_with_basis(
    previous: &Line,
    current: &Line,
) -> (f64, TextSizeBasis, f64, f64) {
    if previous.layout_script == current.layout_script {
        if let (Some(a), Some(b)) = (previous.layout_glyph_height, current.layout_glyph_height) {
            if a > 0.0 && b > 0.0 {
                return (min(a, b) / max(a, b), TextSizeBasis::Glyph, a, b);
            }
        }
    }
    let basis = if previous.layout_script != current.layout_script {
        TextSizeBasis::BoxDifferentScript
    } else {
        TextSizeBasis::BoxNoGlyphHeight
    };
    let a = previous.layout_bounds.height;
    let b = current.layout_bounds.height;
    (min(a, b) / max(a, b), basis, a, b)
}

fn sentence_continuation_evidence(
    previous: &Line,
    current: &Line,
    profile: &GroupingProfile,
    size_ratio: f64,
    loose_prose: bool,
    japanese_prose: bool,
) -> (bool, &'static str) {
    let previous_text = previous.text.trim();
    let current_text = current.text.trim();
    if previous_text.is_empty() || current_text.is_empty() {
        return (false, "empty text");
    }
    if starts_with_list_bullet(current_text) {
        return (false, "list bullet");
    }
    if profile.refuse_speaker_line_starts && speaker::opens(current_text) {
        return (false, "speaker line start");
    }
    if profile.refuse_speaker_line_starts && speaker::is_bare_label(previous_text) {
        return (false, "speaker label");
    }
    if is_numeric_row(current_text) {
        return (false, "numeric row");
    }
    if ends_with_label_colon(previous_text)
        && line_advance_ratio(previous, current) > SOLID_LINE_ADVANCE
    {
        return (false, "label colon");
    }
    if japanese_prose {
        return (true, "Japanese prose rows");
    }
    if size_ratio < MIN_TEXT_SIZE_RATIO && !is_set_solid_under(previous, current, profile) {
        return (false, "text size");
    }
    if has_unclosed_delimiter(previous_text)
        || ends_with_continuation_punctuation(previous_text)
        || starts_with_continuation_punctuation(current_text)
    {
        return (true, "punctuation");
    }
    if ends_with_sentence_terminator(previous_text) {
        return (false, "sentence terminator");
    }
    if is_set_solid_under(previous, current, profile) || loose_prose {
        return (true, "set solid");
    }
    if !is_long_enough_to_have_wrapped(previous)
        || previous.layout_bounds.width < current.layout_bounds.width * 1.35
    {
        return (false, "no continuation evidence");
    }
    if line_advance_ratio(previous, current) <= WRAPPED_FINAL_LINE_ADVANCE {
        (true, "shorter final line")
    } else {
        (false, "leading")
    }
}

fn is_set_solid_under(previous: &Line, current: &Line, profile: &GroupingProfile) -> bool {
    let avg_height = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
    line_advance_ratio(previous, current) <= solid_bar_for(previous, profile)
        && shares_an_edge(previous, current)
        && (is_long_enough_to_have_wrapped(previous)
            || (profile.waive_length_test_when_set_solid
                && is_centred_against(previous, current, avg_height)))
}

fn is_japanese_prose_continuation(
    previous: &Line,
    current: &Line,
    profile: &GroupingProfile,
) -> bool {
    if !profile.takes_prose_evidence
        || !previous.text.encode_utf16().any(line::is_kana)
        || !current.text.encode_utf16().any(line::is_kana)
        || !is_long_enough_to_have_wrapped(previous)
        || !line::spans(current.layout_bounds, line::SUBSTANTIAL_LINE_MIN_ASPECT)
        || !(ends_with_sentence_terminator(previous.text.trim())
            || ends_with_continuation_punctuation(previous.text.trim()))
    {
        return false;
    }
    let a = previous.layout_bounds;
    let b = current.layout_bounds;
    let height = (a.height + b.height) / 2.0;
    let overlap = min(a.right(), b.right()) - max(a.x, b.x);
    text_size_ratio(previous, current) >= profile.tightly_set_min_text_size_ratio
        && line_advance_ratio(previous, current) <= profile.solid_line_advance_when_wrapped
        && alignment_delta(previous, current) <= height * 2.0
        && overlap >= min(a.width, b.width) * 0.5
}

fn is_loosely_set_prose(
    previous: &Line,
    current: &Line,
    profile: &GroupingProfile,
    lines: &[Node],
) -> bool {
    if !profile.takes_prose_evidence
        || !line::spans(previous.layout_bounds, line::PROSE_LINE_MIN_ASPECT)
        || !line::spans(current.layout_bounds, line::WRAPPED_LINE_MIN_ASPECT)
        || !shares_an_edge(previous, current)
        || text_size_ratio(previous, current) < MIN_TEXT_SIZE_RATIO
        || line_advance_ratio(previous, current) > LOOSE_PROSE_MAX_ADVANCE
    {
        return false;
    }
    let mut column: Vec<_> = lines
        .iter()
        .filter(|b| {
            text_size_ratio(previous, b) >= MIN_TEXT_SIZE_RATIO && shares_an_edge(previous, b)
        })
        .collect();
    column.sort_by(|a, b| a.layout_bounds.y.total_cmp(&b.layout_bounds.y));
    let mut advances = Vec::new();
    for pair in column.windows(2) {
        let upper = pair[0];
        let lower = pair[1];
        if !line::spans(upper.layout_bounds, line::WRAPPED_LINE_MIN_ASPECT)
            || line_advance_ratio(upper, lower) < COLUMN_LEADING_SAMPLE_MIN
            || line_advance_ratio(upper, lower) > COLUMN_LEADING_SAMPLE_MAX
        {
            continue;
        }
        advances.push(lower.layout_bounds.y - upper.layout_bounds.y);
    }
    if advances.len() < MIN_COLUMN_LEADING_SAMPLES {
        return false;
    }
    advances.sort_by(f64::total_cmp);
    let typical = advances[(advances.len() - 1) / 4];
    current.layout_bounds.y - previous.layout_bounds.y <= typical * COLUMN_LEADING_SLACK
}

fn is_established_paragraph_final_line(
    group: &[Node],
    current: &Line,
    profile: &GroupingProfile,
) -> bool {
    if !profile.takes_prose_evidence
        || group.len() < 3
        || current.layout_script != LayoutScript::Latin
    {
        return false;
    }
    let text = current.text.trim();
    let lower = text
        .encode_utf16()
        .next()
        .and_then(|c| char::from_u32(u32::from(c)))
        .is_some_and(|c| get_general_category(c) == GeneralCategory::LowercaseLetter);
    if text.is_empty() || !lower || !ends_with_sentence_terminator(text) {
        return false;
    }
    let previous = &group[group.len() - 1];
    let before = &group[group.len() - 2];
    if previous.layout_script != LayoutScript::Latin
        || before.layout_script != LayoutScript::Latin
        || ends_with_sentence_terminator(previous.text.trim())
        || previous.layout_bounds.width
            < group
                .iter()
                .map(|b| b.layout_bounds.width)
                .reduce(max)
                .unwrap()
                * 0.85
        || current.layout_bounds.width > previous.layout_bounds.width * 0.5
        || (previous.layout_bounds.x - current.layout_bounds.x).abs()
            > previous.layout_bounds.height * 0.2
    {
        return false;
    }
    let height_ratio = line::height_ratio(previous.layout_bounds, current.layout_bounds);
    let last_advance = previous.layout_bounds.y - before.layout_bounds.y;
    let advance = current.layout_bounds.y - previous.layout_bounds.y;
    height_ratio >= PARAGRAPH_FINAL_HEIGHT_RATIO
        && text_size_ratio(before, previous) >= MIN_TEXT_SIZE_RATIO
        && last_advance > 0.0
        && advance >= last_advance * 0.85
        && advance <= last_advance * 1.15
}

fn is_centred_against(previous: &Line, current: &Line, avg_height: f64) -> bool {
    let inset = max(avg_height * SET_SOLID_MIN_CENTRING_INSET, 4.0);
    if inset_within(previous, current) >= inset || inset_within(current, previous) >= inset {
        return true;
    }
    (center_x(previous) - center_x(current)).abs()
        <= max(avg_height * SET_SOLID_MAX_CENTRING_OFFSET, 4.0)
        && would_have_wrapped_together(previous, current)
}
fn inset_within(inner: &Line, outer: &Line) -> f64 {
    min(
        inner.layout_bounds.x - outer.layout_bounds.x,
        outer.layout_bounds.right() - inner.layout_bounds.right(),
    )
}
fn would_have_wrapped_together(previous: &Line, current: &Line) -> bool {
    line::aspect(previous.layout_bounds) + line::aspect(current.layout_bounds)
        >= line::WRAPPED_LINE_MIN_ASPECT
}
fn solid_bar_for(previous: &Line, profile: &GroupingProfile) -> f64 {
    if is_long_enough_to_have_wrapped(previous) {
        profile.solid_line_advance_when_wrapped
    } else {
        SOLID_LINE_ADVANCE
    }
}
fn shares_an_edge(previous: &Line, current: &Line) -> bool {
    let avg_height = (previous.layout_bounds.height + current.layout_bounds.height) / 2.0;
    let edges = min(
        (previous.layout_bounds.x - current.layout_bounds.x).abs(),
        (previous.layout_bounds.right() - current.layout_bounds.right()).abs(),
    );
    edges <= max(avg_height * SET_SOLID_MAX_ALIGNMENT, 6.0)
        || (center_x(previous) - center_x(current)).abs()
            <= max(
                avg_height * SET_SOLID_MAX_CENTRE_ALIGNMENT,
                SET_SOLID_MIN_CENTRE_PIXELS,
            )
}
fn is_long_enough_to_have_wrapped(line: &Line) -> bool {
    line::aspect(line.layout_bounds) >= line::WRAPPED_LINE_MIN_ASPECT
}
fn has_unclosed_delimiter(text: &str) -> bool {
    [
        ('「', '」'),
        ('『', '』'),
        ('（', '）'),
        ('(', ')'),
        ('【', '】'),
        ('《', '》'),
        ('〈', '〉'),
    ]
    .iter()
    .any(|&(a, b)| {
        text.chars().filter(|&c| c == a).count() > text.chars().filter(|&c| c == b).count()
    }) || text.chars().filter(|&c| c == '"').count() % 2 == 1
}
fn ends_with_continuation_punctuation(text: &str) -> bool {
    text.ends_with([
        '、', '，', ',', '：', ':', '；', ';', '「', '『', '（', '(', '【', '《', '〈',
    ])
}
fn starts_with_continuation_punctuation(text: &str) -> bool {
    text.starts_with([
        '」', '』', '）', ')', '】', '》', '〉', '、', '，', ',', '。', '！', '？', '!', '?',
    ])
}
fn ends_with_sentence_terminator(text: &str) -> bool {
    text.ends_with(['。', '！', '？', '!', '?', '.'])
}
fn starts_with_list_bullet(text: &str) -> bool {
    text.starts_with(['·', '・', '•', '‧', '●', '○', '▪', '◆', '※'])
}
fn ends_with_label_colon(text: &str) -> bool {
    text.ends_with([':', '：'])
}
fn is_numeric_row(text: &str) -> bool {
    let mut digit = false;
    for c in text.encode_utf16() {
        if line::is_letter(c) {
            return false;
        }
        digit |= line::is_digit(c);
    }
    digit
}
