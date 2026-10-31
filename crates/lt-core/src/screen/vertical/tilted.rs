//! Finds columns that agree on a tilt and builds the cards they sit on.

use super::super::{
    geom::{LineGeometry, PointF, RectF},
    line::Line,
    tilt::TiltedText,
};

/// How far from vertical every agreeing column must be.
pub const FROM_DEGREES: f64 = 4.0;
/// How far apart the agreeing columns' angles may be.
pub const MAX_SPREAD: f64 = 3.0;
/// Past this a column is closer to a diagonal than to writing that runs down the page, and is left
/// upright rather than guessed at.
pub const TO_DEGREES: f64 = 30.0;
/// How long a column's quadrilateral must be against its thickness for its angle to be trusted.
pub const TRUSTED_LENGTH: f64 = 2.0;
/// How long a column must be to say on its own that it is tilted.
pub const LONE_LENGTH: f64 = 5.0;
/// How far a column on its own must be turned.
pub const LONE_FROM_DEGREES: f64 = 6.0;

pub fn deviation(line: LineGeometry) -> f64 {
    if line.angle_degrees > 0.0 {
        line.angle_degrees - 90.0
    } else {
        line.angle_degrees + 90.0
    }
}
pub fn runs_down(line: LineGeometry) -> bool {
    line.angle_degrees.abs() > 45.0
}
pub fn angle(columns: &[LineGeometry], trusted_length: f64) -> Option<f64> {
    let trusted: Vec<_> = columns
        .iter()
        .copied()
        .filter(|l| runs_down(*l) && l.length >= l.thickness * trusted_length)
        .collect();
    if trusted.is_empty() {
        return None;
    }
    if trusted.len() == 1 {
        let line = trusted[0];
        let alone = deviation(line);
        return (line.length >= line.thickness * LONE_LENGTH
            && (LONE_FROM_DEGREES..=TO_DEGREES).contains(&alone.abs()))
        .then_some(alone);
    }
    let angles: Vec<_> = trusted.iter().map(|l| deviation(*l)).collect();
    if angles
        .iter()
        .any(|a| a.abs() < FROM_DEGREES || a.abs() > TO_DEGREES)
        || (angles.iter().any(|a| *a > 0.0) && angles.iter().any(|a| *a < 0.0))
    {
        return None;
    }
    let low = angles.iter().copied().fold(f64::INFINITY, f64::min);
    let high = angles.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if high - low > MAX_SPREAD {
        return None;
    }
    Some(
        trusted
            .iter()
            .map(|l| deviation(*l) * l.length)
            .sum::<f64>()
            / trusted.iter().map(|l| l.length).sum::<f64>(),
    )
}
pub fn quad(box_rect: RectF, line: LineGeometry) -> Vec<PointF> {
    let centre = PointF::new(
        box_rect.x + box_rect.width / 2.0,
        box_rect.y + box_rect.height / 2.0,
    );
    let radians = line.angle_degrees * std::f64::consts::PI / 180.0;
    let along = PointF::new(
        radians.cos() * line.length / 2.0,
        radians.sin() * line.length / 2.0,
    );
    let across = PointF::new(
        -radians.sin() * line.thickness / 2.0,
        radians.cos() * line.thickness / 2.0,
    );
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .map(|(a, b)| {
            PointF::new(
                centre.x + a * along.x + b * across.x,
                centre.y + a * along.y + b * across.y,
            )
        })
        .to_vec()
}
pub fn for_columns(columns: &[Line]) -> Option<TiltedText> {
    if columns.is_empty() {
        return None;
    }
    let geometry: Option<Vec<_>> = columns.iter().map(|c| c.line_geometry).collect();
    let degrees = angle(&geometry?, TRUSTED_LENGTH)?;
    let quads: Vec<_> = columns
        .iter()
        .map(|c| {
            let b = c.layout_bounds;
            quad(
                if !b.is_empty() && b.width > 0.0 && b.height > 0.0 {
                    b
                } else {
                    c.bounds
                },
                c.line_geometry.unwrap(),
            )
        })
        .collect();
    TiltedText::from_columns(degrees, &quads)
}
