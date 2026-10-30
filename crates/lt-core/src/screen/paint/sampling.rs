use super::{
    color, layers,
    vote::DominantColorVote,
    window::{from_edges, PixelWindow},
};
use crate::screen::{
    geom::{PointF, RectF},
    ImageView,
};

const MAJORITY: f64 = 0.6;
const MINIMUM_SAMPLES: usize = 64;

pub fn between_lines(image: ImageView<'_>, lines: &[RectF], vertical: bool) -> Option<[u8; 3]> {
    if image.width == 0 || image.height == 0 {
        return None;
    }
    let mut ordered: Vec<_> = lines
        .iter()
        .copied()
        .filter(|r| r.width > 0.0 && r.height > 0.0 && (r.x + r.y + r.width + r.height).is_finite())
        .collect();
    ordered.sort_by(|a, b| {
        if vertical {
            a.x.total_cmp(&b.x)
        } else {
            a.y.total_cmp(&b.y)
        }
    });
    if ordered.len() < 2 {
        return None;
    }
    let mut vote = DominantColorVote::default();
    let mut samples = 0;
    for pair in ordered.windows(2) {
        let (first, second) = (pair[0], pair[1]);
        let thickness = if vertical {
            first.width.min(second.width)
        } else {
            first.height.min(second.height)
        };
        let inset = (thickness * 0.15).clamp(2.0, 6.0);
        let (left, right, top, bottom) = if vertical {
            (
                first.right() + inset,
                second.x - inset,
                first.y.max(second.y),
                first.bottom().min(second.bottom()),
            )
        } else {
            (
                first.x.max(second.x),
                first.right().min(second.right()),
                first.bottom() + inset,
                second.y - inset,
            )
        };
        let gap = from_edges(
            left.ceil().clamp(0.0, f64::from(image.width)) as i32,
            top.ceil().clamp(0.0, f64::from(image.height)) as i32,
            right.floor().clamp(0.0, f64::from(image.width)) as i32,
            bottom.floor().clamp(0.0, f64::from(image.height)) as i32,
        );
        let Some(pixels) = PixelWindow::read(image, gap) else {
            continue;
        };
        let step = ((f64::from(gap.w) * f64::from(gap.h) / 2048.0).sqrt().ceil() as usize).max(1);
        for y in (gap.y..gap.bottom()).step_by(step) {
            for x in (gap.x..gap.right()).step_by(step) {
                if ordered
                    .iter()
                    .any(|line| line.contains(PointF::new(f64::from(x), f64::from(y))))
                {
                    continue;
                }
                vote.add(pixels.at(x, y));
                samples += 1;
            }
        }
    }
    if samples < MINIMUM_SAMPLES {
        return None;
    }
    vote.dominant_with_share()
        .filter(|(_, share)| *share >= MAJORITY)
        .map(|(rgb, _)| rgb)
}

pub fn sample(
    image: ImageView<'_>,
    bounds: RectF,
    background_override: Option<[u8; 3]>,
    ring_reference: Option<f64>,
) -> Option<layers::TextLayers> {
    if image.width == 0 || image.height == 0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }
    let inner = from_edges(
        (bounds.x as i32).clamp(0, image.width as i32),
        (bounds.y as i32).clamp(0, image.height as i32),
        (bounds.right() as i32).clamp(0, image.width as i32),
        (bounds.bottom() as i32).clamp(0, image.height as i32),
    );
    if inner.w == 0 || inner.h == 0 {
        return None;
    }
    let reference = ring_reference.filter(|r| *r > 0.0).unwrap_or(bounds.height);
    let pad_x = 4.max((reference * 0.35).round_ties_even() as i32);
    let pad_y = 3.max((reference * 0.28).round_ties_even() as i32);
    let outer = from_edges(
        (bounds.x as i32 - pad_x).clamp(0, image.width as i32),
        (bounds.y as i32 - pad_y).clamp(0, image.height as i32),
        (bounds.right() as i32 + pad_x).clamp(0, image.width as i32),
        (bounds.bottom() as i32 + pad_y).clamp(0, image.height as i32),
    );
    let window = PixelWindow::read(image, outer)?;
    Some(layers::read(&window, inner, background_override))
}

pub fn for_capture_overlay(
    image: ImageView<'_>,
    bounds: RectF,
    source_lines: Option<&[RectF]>,
    vertical: bool,
) -> ([u8; 3], [u8; 3]) {
    let paper = source_lines
        .filter(|lines| lines.len() > 1)
        .and_then(|lines| between_lines(image, lines, vertical));
    let ring = source_lines.and_then(|lines| {
        lines
            .iter()
            .map(|line| if vertical { line.width } else { line.height })
            .reduce(f64::min)
    });
    let Some(sample) = sample(image, bounds, paper, ring) else {
        return ([255; 3], [0; 3]);
    };
    let background = sample.surface;
    let Some(text) = sample.body else {
        return (
            background,
            if color::perceived_luminance(background) > 0.5 {
                [0; 3]
            } else {
                [255; 3]
            },
        );
    };
    let mut tuned = color::tune(text, background);
    if let Some(outline) = sample.outline {
        if color::contrast_ratio(tuned, background) < color::MINIMUM_CONTRAST
            && color::contrast_ratio(outline, background) > color::contrast_ratio(tuned, background)
        {
            tuned = color::tune(outline, background);
        }
    }
    (
        background,
        color::ensure_contrast(tuned, background, color::MINIMUM_CONTRAST),
    )
}
