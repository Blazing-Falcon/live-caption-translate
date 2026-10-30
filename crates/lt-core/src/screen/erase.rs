use super::{
    geom::{PointF, RectF},
    imaging::{
        arithmetic::{self, Operation},
        channels, color_conversion, conversion, distance_transform, filters, inpaint,
        morphology::{self, StructuringElement},
        resize::{self, Interpolation},
        statistics, threshold, ImageBuffer, PixelType,
    },
    ImageView,
};
use crate::error::{Error, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

const BOX_PADDING: f64 = 0.09;
const OUTLINE_TAIL: f64 = 10.0;
const REACH: f64 = 0.20;
const FURTHEST: i32 = 7;
const FURTHEST_HALO: i32 = 2;
const WIDEST_KERNEL: usize = 25;
const SCALED_FROM: usize = 41;
const FILL_RADIUS: f64 = 3.0;
const SLOPE_DEPARTURE: f64 = 8.0;
const STRUCTURE_DEPARTURE: f64 = 30.0;
const DEPARTURE_WINDOW: usize = 97;
const TILE: usize = 96;
const MARGIN: usize = TILE;
const GRAIN_WINDOW: usize = 33;
const GRAIN_QUIET: f64 = 12.0;
const GRAIN_CEILING: f64 = 9.0;
const GRAIN_EXPECTATION: f64 = 0.7979;
const SLOPE_SCALE: usize = 2;
const DONOR_GUARD: usize = 2;
const SMOOTH_SCALE: usize = 4;
const SPECKLE_SIDE: usize = 256;

pub struct TextRegion {
    pub bounds: RectF,
    pub glyph_height: Option<f64>,
    pub within: Option<Vec<Vec<PointF>>>,
}
pub struct RemovalBlock {
    pub translated_text: String,
    pub bounds: RectF,
    pub source_line_bounds: Option<Vec<RectF>>,
    pub glyph_height: Option<f64>,
    pub within: Option<Vec<Vec<PointF>>>,
}
pub struct CpuRepair {
    pub image: ImageBuffer,
    pub full_tiles: usize,
    pub reduced_tiles: usize,
}
struct Reduction {
    sum: ImageBuffer,
    seen: ImageBuffer,
}
type Area = (usize, usize, usize, usize);
fn view(image: &ImageBuffer, a: Area) -> ImageBuffer {
    image.view(a.0, a.1, a.2, a.3)
}
fn blank(image: &ImageBuffer, pixel_type: PixelType) -> ImageBuffer {
    ImageBuffer::new(image.width, image.height, pixel_type)
}
fn clip(left: f64, top: f64, right: f64, bottom: f64, width: usize, height: usize) -> Area {
    let x = left.floor().clamp(0.0, width as f64) as usize;
    let y = top.floor().clamp(0.0, height as f64) as usize;
    let r = right.ceil().clamp(x as f64, width as f64) as usize;
    let b = bottom.ceil().clamp(y as f64, height as f64) as usize;
    (x, y, r - x, b - y)
}
fn expand(a: Area, margin: usize, width: usize, height: usize) -> Area {
    let x = a.0.saturating_sub(margin);
    let y = a.1.saturating_sub(margin);
    (
        x,
        y,
        (a.0 + a.2 + margin).min(width) - x,
        (a.1 + a.3 + margin).min(height) - y,
    )
}
fn local(a: Area, within: Area) -> Area {
    (a.0 - within.0, a.1 - within.1, a.2, a.3)
}
fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Stopped)
    } else {
        Ok(())
    }
}

pub fn erase_text(frame: &ImageView<'_>, blocks: &[RemovalBlock]) -> Vec<u8> {
    erase_text_cancellable(frame, blocks, &AtomicBool::new(false)).unwrap()
}
pub fn erase_text_cancellable(
    frame: &ImageView<'_>,
    blocks: &[RemovalBlock],
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    check(cancel)?;
    let regions: Vec<_> = blocks
        .iter()
        .filter(|b| !b.translated_text.trim().is_empty())
        .flat_map(|b| {
            b.source_line_bounds
                .as_deref()
                .filter(|lines| !lines.is_empty())
                .unwrap_or(std::slice::from_ref(&b.bounds))
                .iter()
                .map(|bounds| TextRegion {
                    bounds: *bounds,
                    glyph_height: b.glyph_height,
                    within: b.within.clone(),
                })
        })
        .collect();
    if regions.is_empty() {
        return Ok(frame_bytes(frame));
    }
    let source = ImageBuffer::new(frame.width as usize, frame.height as usize, PixelType::U8C3);
    for y in 0..source.height {
        for x in 0..source.width {
            for c in 0..3 {
                source.set_byte(
                    y,
                    x * 3 + c,
                    frame.bgra[y * frame.stride as usize + x * 4 + c],
                );
            }
        }
    }
    let mask = build_mask(&source, &regions);
    let repaired = repair_with_cancel(&source, &mask, cancel)?;
    check(cancel)?;
    let mut output = frame_bytes(frame);
    for y in 0..source.height {
        for x in 0..source.width {
            for c in 0..3 {
                output[(y * source.width + x) * 4 + c] = repaired.image.byte(y, x * 3 + c);
            }
            output[(y * source.width + x) * 4 + 3] = 255;
        }
    }
    Ok(output)
}
fn frame_bytes(frame: &ImageView<'_>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(frame.width as usize * frame.height as usize * 4);
    for y in 0..frame.height as usize {
        bytes.extend_from_slice(
            &frame.bgra
                [y * frame.stride as usize..y * frame.stride as usize + frame.width as usize * 4],
        );
    }
    bytes
}
fn room(index: usize, lines: &[TextRegion]) -> i32 {
    let line = &lines[index].bounds;
    let mut nearest = f64::MAX;
    for (i, other) in lines.iter().enumerate() {
        let other = &other.bounds;
        if i == index || other.width <= 0.0 || other.height <= 0.0 {
            continue;
        }
        let shared = line.right().min(other.right()) - line.x.max(other.x);
        if shared < line.width.min(other.width) * 0.2 {
            continue;
        }
        let apart = if other.y >= line.bottom() {
            other.y - line.bottom()
        } else if line.y >= other.bottom() {
            line.y - other.bottom()
        } else {
            0.0
        };
        nearest = nearest.min(apart);
    }
    if nearest == f64::MAX {
        i32::MAX
    } else {
        (nearest / 2.0).floor() as i32
    }
}
pub fn build_mask(source: &ImageBuffer, lines: &[TextRegion]) -> ImageBuffer {
    let mask = blank(source, PixelType::U8C1);
    let gray = color_conversion::to_gray(source);
    let one = StructuringElement::ellipse(3, 3);
    for (index, line) in lines.iter().enumerate() {
        let b = line.bounds;
        if !(b.x + b.y + b.width + b.height).is_finite() || b.width <= 0.0 || b.height <= 0.0 {
            continue;
        }
        let height = 1.0f64.max(line.glyph_height.unwrap_or(b.height).min(b.height));
        let padding = (height * BOX_PADDING).ceil().clamp(2.0, 4.0);
        let allowance = FURTHEST.min(
            2.max((height * REACH).round_ties_even() as i32 + 1)
                .min(2.max(room(index, lines))),
        );
        let halo = (f64::from(allowance) * 0.3)
            .round_ties_even()
            .clamp(1.0, f64::from(FURTHEST_HALO)) as usize;
        let growth = (allowance - 1 - halo as i32).max(0) as usize;
        let area = clip(
            b.x - padding,
            b.y - padding,
            b.right() + padding,
            b.bottom() + padding,
            source.width,
            source.height,
        );
        if area.2 == 0 || area.3 == 0 {
            continue;
        }
        let roi = view(&gray, area);
        let light = blank(&roi, PixelType::U8C1);
        let dark = blank(&roi, PixelType::U8C1);
        hats(&roi, height, &light, &dark);
        let seed = blank(&roi, PixelType::U8C1);
        let darker = blank(&roi, PixelType::U8C1);
        body(&light, &seed);
        body(&dark, &darker);
        arithmetic::binary(&seed, &darker, &seed, Operation::Or);
        let strongest = blank(&roi, PixelType::U8C1);
        arithmetic::binary(&light, &dark, &strongest, Operation::Max);
        if growth > 0 {
            grow_along_tail(&seed, &strongest, &one, growth);
        }
        morphology::dilate(&seed, &seed, &one, 1);
        let wide = expand(area, halo, source.width, source.height);
        let spread = ImageBuffer::new(wide.2, wide.3, PixelType::U8C1);
        seed.copy_to(&spread.view(area.0 - wide.0, area.1 - wide.1, area.2, area.3));
        morphology::dilate(&spread, &spread, &one, halo);
        if let Some(quads) = &line.within {
            keep_inside(&spread, wide, quads);
        }
        let target = view(&mask, wide);
        arithmetic::binary(&target, &spread, &target, Operation::Or);
    }
    mask
}
fn hats(roi: &ImageBuffer, height: f64, light: &ImageBuffer, dark: &ImageBuffer) {
    let wanted = (height * 0.55).round_ties_even() as usize | 1;
    if wanted < SCALED_FROM || roi.width < 4 || roi.height < 4 {
        let kernel = StructuringElement::ellipse(
            wanted.clamp(5, WIDEST_KERNEL),
            wanted.clamp(5, WIDEST_KERNEL),
        );
        morphology::top_hat(roi, light, &kernel);
        morphology::black_hat(roi, dark, &kernel);
        return;
    }
    let scale = WIDEST_KERNEL as f64 / wanted as f64;
    let w = (roi.width as f64 * scale).round_ties_even().max(2.0) as usize;
    let h = (roi.height as f64 * scale).round_ties_even().max(2.0) as usize;
    let small = resize::to(roi, w, h, Interpolation::Area);
    let widest = StructuringElement::ellipse(WIDEST_KERNEL, WIDEST_KERNEL);
    let small_light = blank(&small, PixelType::U8C1);
    let small_dark = blank(&small, PixelType::U8C1);
    morphology::top_hat(&small, &small_light, &widest);
    morphology::black_hat(&small, &small_dark, &widest);
    resize::to_buffer(&small_light, light, Interpolation::Linear);
    resize::to_buffer(&small_dark, dark, Interpolation::Linear);
}
fn body(response: &ImageBuffer, target: &ImageBuffer) {
    let otsu = threshold::otsu(response);
    threshold::binary(response, target, 18.0f64.max(otsu * 0.8), 255.0);
}
fn grow_along_tail(
    seed: &ImageBuffer,
    response: &ImageBuffer,
    one: &StructuringElement,
    steps: usize,
) {
    let tail = blank(response, PixelType::U8C1);
    threshold::binary(response, &tail, OUTLINE_TAIL, 255.0);
    let grown = blank(seed, PixelType::U8C1);
    for _ in 0..steps {
        morphology::dilate(seed, &grown, one, 1);
        arithmetic::binary(&grown, &tail, &grown, Operation::And);
        arithmetic::binary(&grown, seed, seed, Operation::Or);
    }
}
fn across(polygon: &[PointF], y: f64) -> Option<(f64, f64)> {
    let (mut left, mut right) = (f64::MAX, f64::MIN);
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        if (a.y > y && b.y > y) || (a.y < y && b.y < y) {
            continue;
        }
        if a.y == b.y {
            left = left.min(a.x.min(b.x));
            right = right.max(a.x.max(b.x));
            continue;
        }
        let x = a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x);
        left = left.min(x);
        right = right.max(x);
    }
    if left <= right {
        Some((left, right))
    } else {
        None
    }
}
fn keep_inside(spread: &ImageBuffer, area: Area, quads: &[Vec<PointF>]) {
    for y in 0..spread.height {
        let py = (area.1 + y) as f64 + 0.5;
        let mut runs = Vec::new();
        for quad in quads {
            if let Some((left, right)) = across(quad, py) {
                let from = 0.max((left - area.0 as f64 - 0.5).ceil() as i32);
                let to =
                    (spread.width as i32 - 1).min((right - area.0 as f64 - 0.5).floor() as i32);
                if from <= to {
                    runs.push((from as usize, to as usize));
                }
            }
        }
        runs.sort();
        let mut kept = 0;
        for (from, to) in runs {
            for x in kept..from {
                spread.set_byte(y, x, 0);
            }
            kept = kept.max(to + 1);
        }
        for x in kept..spread.width {
            spread.set_byte(y, x, 0);
        }
    }
}

pub fn repair(source: &ImageBuffer, mask: &ImageBuffer) -> CpuRepair {
    repair_with_cancel(source, mask, &AtomicBool::new(false)).unwrap()
}
pub fn repair_with_cancel(
    source: &ImageBuffer,
    mask: &ImageBuffer,
    cancel: &AtomicBool,
) -> Result<CpuRepair> {
    check(cancel)?;
    assert_eq!(source.pixel_type, PixelType::U8C3);
    source.require_mask(mask);
    let holes = statistics::bounding_rect(mask);
    if holes.2 == 0 || holes.3 == 0 {
        return Ok(CpuRepair {
            image: source.isolated(),
            full_tiles: 0,
            reduced_tiles: 0,
        });
    }
    let area = expand(holes, MARGIN, source.width, source.height);
    let frame = view(source, area);
    let within = view(mask, area);
    let reduced = reduce(&frame, &within);
    let slope = slope_share(&reduced, frame.width, frame.height);
    let (least, most) = statistics::min_max(&slope, Some(&within));
    let least = least / 255.0;
    let most = most / 255.0;
    check(cancel)?;
    if most <= 0.02 {
        return Ok(grained(
            structured(source, mask, cancel, None, None, area)?,
            source,
            mask,
            area,
        ));
    }
    let smooth = smooth_fill(&reduced, frame.width, frame.height);
    check(cancel)?;
    if least >= 0.98 {
        let only = source.isolated();
        smooth.copy_masked(&view(&only, area), &within);
        return Ok(grained(
            CpuRepair {
                image: only,
                full_tiles: 0,
                reduced_tiles: 0,
            },
            source,
            mask,
            area,
        ));
    }
    Ok(grained(
        structured(source, mask, cancel, Some(&smooth), Some(&slope), area)?,
        source,
        mask,
        area,
    ))
}
fn grained(repair: CpuRepair, source: &ImageBuffer, mask: &ImageBuffer, area: Area) -> CpuRepair {
    let outside = blank(mask, PixelType::U8C1);
    arithmetic::not(mask, &outside);
    let smoothed = repair.image.isolated();
    let guard = GRAIN_WINDOW / 2 + 1;
    let span = TILE * 2;
    for y in (area.1..area.1 + area.3).step_by(span) {
        for x in (area.0..area.0 + area.2).step_by(span) {
            let block = (
                x,
                y,
                span.min(area.0 + area.2 - x),
                span.min(area.1 + area.3 - y),
            );
            let covered = view(mask, block);
            if statistics::count_non_zero(&covered) == 0 {
                continue;
            }
            let crop = expand(block, guard, source.width, source.height);
            let frame = view(source, crop);
            let filled = view(&smoothed, crop);
            let beyond = view(&outside, crop);
            let within = view(mask, crop);
            let ambient = energy(&frame, &beyond);
            let present = energy(&filled, &within);
            let strength = blank(&frame, PixelType::F32C1);
            arithmetic::binary(&ambient, &present, &strength, Operation::Subtract);
            arithmetic::scalar(&strength, 0.0, &strength, Operation::Max);
            arithmetic::scalar(
                &strength,
                GRAIN_CEILING * GRAIN_EXPECTATION,
                &strength,
                Operation::Min,
            );
            arithmetic::scalar(&strength, GRAIN_EXPECTATION, &strength, Operation::Divide);
            let noise = noise(crop);
            arithmetic::binary(&noise, &strength, &noise, Operation::Multiply);
            let grain = channels::spread(&noise);
            let grained = blank(&frame, PixelType::U8C3);
            arithmetic::add_to_bytes(&filled, &grain, &grained);
            let interior = grained.view(block.0 - crop.0, block.1 - crop.1, block.2, block.3);
            interior.copy_masked(&view(&repair.image, block), &covered);
        }
    }
    repair
}
fn energy(image: &ImageBuffer, where_: &ImageBuffer) -> ImageBuffer {
    let gray = color_conversion::to_gray(image);
    let average = blank(&gray, PixelType::U8C1);
    filters::blur(&gray, &average, 5);
    let detail = blank(&gray, PixelType::U8C1);
    arithmetic::binary(&gray, &average, &detail, Operation::AbsoluteDifference);
    arithmetic::scalar(&detail, GRAIN_QUIET, &detail, Operation::Min);
    let answered = blank(&detail, PixelType::U8C1);
    detail.copy_masked(&answered, where_);
    let carried = filters::box_sum(&answered, GRAIN_WINDOW);
    let seen = filters::box_sum(where_, GRAIN_WINDOW);
    arithmetic::scalar(&seen, 1e-3, &seen, Operation::Max);
    let energy = blank(&seen, PixelType::F32C1);
    arithmetic::divide(&carried, &seen, &energy, 255.0);
    energy
}
fn speckle() -> &'static [f32] {
    static VALUES: OnceLock<Vec<f32>> = OnceLock::new();
    VALUES.get_or_init(|| {
        let mut values = vec![0.0; SPECKLE_SIDE * SPECKLE_SIDE];
        let mut state = 0x9E3779B97F4A7C15u64;
        for i in (0..values.len()).step_by(2) {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let first = ((state >> 11) + 1) as f64 / (1u64 << 53) as f64;
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let second = (state >> 11) as f64 / (1u64 << 53) as f64;
            let radius = (-2.0 * first.ln()).sqrt();
            values[i] = (radius * (2.0 * std::f64::consts::PI * second).cos()) as f32;
            if i + 1 < values.len() {
                values[i + 1] = (radius * (2.0 * std::f64::consts::PI * second).sin()) as f32;
            }
        }
        values
    })
}
fn noise(crop: Area) -> ImageBuffer {
    let field = speckle();
    let image = ImageBuffer::new(crop.2, crop.3, PixelType::F32C1);
    let left = crop.0 % SPECKLE_SIDE;
    let top = crop.1 % SPECKLE_SIDE;
    for y in 0..crop.3 {
        let from = ((top + y) % SPECKLE_SIDE) * SPECKLE_SIDE;
        for x in 0..crop.2 {
            image.set_float(y, x, field[from + (left + x) % SPECKLE_SIDE]);
        }
    }
    image
}
fn structured(
    source: &ImageBuffer,
    mask: &ImageBuffer,
    cancel: &AtomicBool,
    smooth: Option<&ImageBuffer>,
    share: Option<&ImageBuffer>,
    within: Area,
) -> Result<CpuRepair> {
    let result = source.isolated();
    let mut coarse = None;
    let distance = distance_transform::chamfer3(mask);
    let (mut full, mut reduced) = (0, 0);
    let guard = 24;
    for y in (0..source.height).step_by(TILE) {
        for x in (0..source.width).step_by(TILE) {
            check(cancel)?;
            let area = (
                x,
                y,
                TILE.min(source.width - x),
                TILE.min(source.height - y),
            );
            let tile_mask = view(mask, area);
            if statistics::count_non_zero(&tile_mask) == 0 {
                continue;
            }
            if let (Some(smooth), Some(share)) = (smooth, share) {
                let local = local(area, within);
                if statistics::mean(&view(share, local), Some(&tile_mask)) >= 250.0 {
                    view(smooth, local).copy_masked(&view(&result, area), &tile_mask);
                    continue;
                }
            }
            let (_, radius) = statistics::min_max(&view(&distance, area), None);
            let context = expand(area, guard, source.width, source.height);
            let frame = view(source, context);
            let holes = view(mask, context);
            let blind = statistics::count_non_zero(&holes) == holes.width * holes.height;
            if !blind && (radius <= 5.0 || context.2.min(context.3) < 8) {
                let filled = inpaint::navier_stokes(&frame, &holes, FILL_RADIUS);
                full += 1;
                let interior = filled.view(x - context.0, y - context.1, area.2, area.3);
                let fill = blend(&interior, smooth, share, area, within);
                fill.copy_masked(&view(&result, area), &tile_mask);
            } else {
                let coarse = coarse.get_or_insert_with(|| repair_reduced(source, mask));
                let fill = blend(&view(coarse, area), smooth, share, area, within);
                fill.copy_masked(&view(&result, area), &tile_mask);
                reduced += 1;
            }
        }
    }
    check(cancel)?;
    Ok(CpuRepair {
        image: result,
        full_tiles: full,
        reduced_tiles: reduced,
    })
}
fn blend(
    structured: &ImageBuffer,
    smooth: Option<&ImageBuffer>,
    share: Option<&ImageBuffer>,
    area: Area,
    within: Area,
) -> ImageBuffer {
    let (Some(smooth), Some(share)) = (smooth, share) else {
        return structured.isolated();
    };
    let local = local(area, within);
    let share_tile = view(share, local);
    if statistics::mean(&share_tile, None) <= 2.0 {
        return structured.isolated();
    }
    mix(structured, &view(smooth, local), &share_tile)
}
fn reduce(source: &ImageBuffer, mask: &ImageBuffer) -> Reduction {
    let spread = blank(mask, PixelType::U8C1);
    let ring = StructuringElement::ellipse(DONOR_GUARD * 2 + 1, DONOR_GUARD * 2 + 1);
    morphology::dilate(mask, &spread, &ring, 1);
    let known = blank(mask, PixelType::U8C1);
    arithmetic::not(&spread, &known);
    let held = source.isolated();
    held.set_to(0.0, Some(&spread));
    let size = (
        (source.width / SLOPE_SCALE).max(1),
        (source.height / SLOPE_SCALE).max(1),
    );
    let held_small = resize::to(&held, size.0, size.1, Interpolation::Area);
    let known_small = resize::to(&known, size.0, size.1, Interpolation::Area);
    Reduction {
        sum: conversion::convert(&held_small, PixelType::F32C3, 1.0),
        seen: conversion::convert(&known_small, PixelType::F32C1, 1.0 / 255.0),
    }
}
fn slope_share(reduced: &Reduction, width: usize, height: usize) -> ImageBuffer {
    let colour = average(&reduced.sum, &reduced.seen);
    let value = color_conversion::to_gray(&colour);
    let departure = filters::laplacian(&value);
    arithmetic::absolute(&departure, &departure);
    let whole = blank(&reduced.seen, PixelType::F32C1);
    threshold::binary(&reduced.seen, &whole, 0.9, 1.0);
    let weight = blank(&whole, PixelType::F32C1);
    morphology::erode(&whole, &weight, &StructuringElement::rectangle(3, 3), 1);
    arithmetic::binary(&departure, &weight, &departure, Operation::Multiply);
    filters::blur(&departure, &departure, 3);
    let side = ((DEPARTURE_WINDOW / SLOPE_SCALE / 4) | 1).max(3);
    let strongest = blank(&departure, PixelType::F32C1);
    morphology::dilate(
        &departure,
        &strongest,
        &StructuringElement::rectangle(5, 5),
        1,
    );
    let reach = resize::to(
        &strongest,
        (departure.width / 4).max(1),
        (departure.height / 4).max(1),
        Interpolation::Nearest,
    );
    morphology::dilate(
        &reach,
        &reach,
        &StructuringElement::rectangle(side, side),
        1,
    );
    arithmetic::scalar(&reach, SLOPE_DEPARTURE, &reach, Operation::Subtract);
    arithmetic::scalar(
        &reach,
        STRUCTURE_DEPARTURE - SLOPE_DEPARTURE,
        &reach,
        Operation::Divide,
    );
    arithmetic::scalar(&reach, 1.0, &reach, Operation::Min);
    arithmetic::scalar(&reach, 0.0, &reach, Operation::Max);
    arithmetic::scalar(&reach, 1.0, &reach, Operation::ReverseSubtract);
    let levels = conversion::convert(&reach, PixelType::U8C1, 255.0);
    resize::to(&levels, width, height, Interpolation::Linear)
}
fn smooth_fill(reduced: &Reduction, width: usize, height: usize) -> ImageBuffer {
    let start = (
        (reduced.sum.width * SLOPE_SCALE / SMOOTH_SCALE).max(1),
        (reduced.sum.height * SLOPE_SCALE / SMOOTH_SCALE).max(1),
    );
    let mut sums = vec![resize::to(
        &reduced.sum,
        start.0,
        start.1,
        Interpolation::Area,
    )];
    let mut seen = vec![resize::to(
        &reduced.seen,
        start.0,
        start.1,
        Interpolation::Area,
    )];
    while sums.last().unwrap().width > 2 && sums.last().unwrap().height > 2 {
        let last = seen.last().unwrap();
        if statistics::min_max(last, None).0 > 0.0 {
            break;
        }
        let last = sums.last().unwrap();
        let size = ((last.width / 2).max(1), (last.height / 2).max(1));
        sums.push(resize::to(last, size.0, size.1, Interpolation::Area));
        seen.push(resize::to(
            seen.last().unwrap(),
            size.0,
            size.1,
            Interpolation::Area,
        ));
    }
    let mut estimate = average(sums.last().unwrap(), seen.last().unwrap());
    for level in (0..sums.len() - 1).rev() {
        let up = resize::to(
            &estimate,
            sums[level].width,
            sums[level].height,
            Interpolation::Linear,
        );
        let own = average(&sums[level], &seen[level]);
        let share = blank(&seen[level], PixelType::F32C1);
        arithmetic::scalar(&seen[level], 1.0, &share, Operation::Min);
        estimate = mix(&up, &own, &share);
    }
    let levels = conversion::convert(&estimate, PixelType::U8C3, 1.0);
    resize::to(&levels, width, height, Interpolation::Linear)
}
fn average(sum: &ImageBuffer, seen: &ImageBuffer) -> ImageBuffer {
    let safe = blank(seen, PixelType::F32C1);
    arithmetic::scalar(seen, 1e-6, &safe, Operation::Max);
    let spread = channels::spread(&safe);
    let average = blank(sum, PixelType::F32C3);
    arithmetic::divide(sum, &spread, &average, 1.0);
    average
}
fn mix(first: &ImageBuffer, second: &ImageBuffer, share: &ImageBuffer) -> ImageBuffer {
    let scale = if share.pixel_type == PixelType::U8C1 {
        conversion::convert(share, PixelType::F32C1, 1.0 / 255.0)
    } else {
        share.isolated()
    };
    let towards = channels::spread(&scale);
    let away = blank(&towards, PixelType::F32C3);
    arithmetic::scalar(&towards, 1.0, &away, Operation::ReverseSubtract);
    let a = conversion::convert(first, PixelType::F32C3, 1.0);
    let b = conversion::convert(second, PixelType::F32C3, 1.0);
    arithmetic::binary(&a, &away, &a, Operation::Multiply);
    arithmetic::binary(&b, &towards, &b, Operation::Multiply);
    arithmetic::binary(&a, &b, &a, Operation::Add);
    if first.pixel_type == PixelType::U8C3 {
        conversion::convert(&a, PixelType::U8C3, 1.0)
    } else {
        a.isolated()
    }
}
pub fn repair_reduced(source: &ImageBuffer, mask: &ImageBuffer) -> ImageBuffer {
    let expanded = blank(mask, PixelType::U8C1);
    morphology::dilate(mask, &expanded, &StructuringElement::ellipse(3, 3), 1);
    let size = ((source.width / 2).max(1), (source.height / 2).max(1));
    let small = resize::to(source, size.0, size.1, Interpolation::Area);
    let small_mask = resize::to(&expanded, size.0, size.1, Interpolation::Area);
    threshold::binary(&small_mask, &small_mask, 0.0, 255.0);
    let filled = inpaint::navier_stokes(&small, &small_mask, FILL_RADIUS);
    let full = resize::to(&filled, source.width, source.height, Interpolation::Linear);
    let result = source.isolated();
    full.copy_masked(&result, mask);
    result
}
