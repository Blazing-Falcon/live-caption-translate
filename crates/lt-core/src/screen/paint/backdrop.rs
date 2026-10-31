use super::{color, vote::DominantColorVote};
use crate::screen::tilt::TiltedText;
use crate::screen::{
    geom::{PointF, RectF},
    imaging::{
        arithmetic, channels, filters,
        resize::{self, Interpolation},
        ImageBuffer, PixelType,
    },
};
use std::collections::BTreeMap;
const BLUR_FACTOR: f64 = 0.30;
const FEATHER_FACTOR: f64 = 0.35;
const FEATHER_MINIMUM: f64 = 3.0;
const FEATHER_MAXIMUM: f64 = 12.0;
const WASH_OPACITY: f64 = 0.25;
const PLATE_CONTRAST: f64 = 4.5;
const COMFORTABLE_CONTRAST: f64 = 7.0;
const LIFT_MARGIN: f64 = 1.15;
const SHARP_NEIGHBOURHOOD: f64 = 0.82;
const NEIGHBOURHOOD_GLYPHS: f64 = 2.0;
const SMOOTH: i32 = 2;
const SHARP_FLAT_SHARE: f64 = 0.45;
const SOFT_FLAT_SHARE: f64 = 0.85;
type Area = (i32, i32, i32, i32);
pub struct Backdrop {
    bgr: Vec<u8>,
    width: usize,
    height: usize,
}
pub struct Plate {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
    pub text: [u8; 3],
}
pub fn feather(glyph_height: f64) -> f64 {
    (glyph_height * FEATHER_FACTOR).clamp(FEATHER_MINIMUM, FEATHER_MAXIMUM)
}
fn round(area: RectF) -> Area {
    if !(area.x + area.y + area.width + area.height).is_finite()
        || area.x.abs() > 1e6
        || area.y.abs() > 1e6
        || area.width > 1e6
        || area.height > 1e6
    {
        return (0, 0, 0, 0);
    }
    let left = area.x.floor() as i32;
    let top = area.y.floor() as i32;
    (
        left,
        top,
        area.right().ceil() as i32 - left,
        area.bottom().ceil() as i32 - top,
    )
}
impl Backdrop {
    pub fn create(
        image: crate::screen::ImageView<'_>,
        blocks: &[crate::screen::erase::RemovalBlock],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> crate::error::Result<Option<Self>> {
        if image.width == 0 || image.height == 0 || blocks.is_empty() {
            return Ok(None);
        }
        let pixels = crate::screen::erase::erase_text_cancellable(&image, blocks, cancel)?;
        let repaired =
            ImageBuffer::new(image.width as usize, image.height as usize, PixelType::U8C3);
        for y in 0..image.height as usize {
            for x in 0..image.width as usize {
                for c in 0..3 {
                    repaired.set_byte(y, x * 3 + c, pixels[(y * image.width as usize + x) * 4 + c]);
                }
            }
        }
        Ok(Self::from_repaired(&repaired))
    }

    pub fn card(&self, bubble: RectF, text: [u8; 3]) -> Option<([u8; 3], [u8; 3])> {
        let r = self.clip(round(bubble));
        if r.2 < 2 || r.3 < 2 {
            return None;
        }
        let mut vote = DominantColorVote::default();
        let step = ((f64::from(r.2) * f64::from(r.3) / 4096.0).sqrt().ceil() as usize).max(1);
        for y in (r.1..r.1 + r.3).step_by(step) {
            for x in (r.0..r.0 + r.2).step_by(step) {
                let i = (y as usize * self.width + x as usize) * 3;
                vote.add([self.bgr[i + 2], self.bgr[i + 1], self.bgr[i]]);
            }
        }
        let background = vote.dominant()?;
        Some((
            background,
            legible(&[background], text, color::MINIMUM_CONTRAST),
        ))
    }

    pub fn from_repaired(image: &ImageBuffer) -> Option<Self> {
        assert_eq!(image.pixel_type, PixelType::U8C3);
        if image.is_empty() {
            None
        } else {
            Some(Self {
                bgr: image.bytes(),
                width: image.width,
                height: image.height,
            })
        }
    }
    fn clip(&self, a: Area) -> Area {
        let left = a.0.clamp(0, self.width as i32);
        let top = a.1.clamp(0, self.height as i32);
        (
            left,
            top,
            (a.0 + a.2).clamp(left, self.width as i32) - left,
            (a.1 + a.3).clamp(top, self.height as i32) - top,
        )
    }
    fn read(&self, a: Area) -> ImageBuffer {
        let image = ImageBuffer::new(a.2 as usize, a.3 as usize, PixelType::U8C3);
        for y in 0..image.height {
            for x in 0..image.width * 3 {
                image.set_byte(
                    y,
                    x,
                    self.bgr[((a.1 as usize + y) * self.width + a.0 as usize) * 3 + x],
                );
            }
        }
        image
    }
    pub fn crispness(&self, area: RectF, glyph_height: f64) -> f64 {
        let reach = 4.0f64.max(glyph_height * NEIGHBOURHOOD_GLYPHS);
        let r = self.clip(round(RectF::new(
            area.x - reach,
            area.y - reach,
            area.width + reach * 2.0,
            area.height + reach * 2.0,
        )));
        if r.2 < 2 || r.3 < 2 {
            return 1.0;
        }
        let (mut same, mut total) = (0u64, 0u64);
        for y in r.1..r.1 + r.3 {
            for x in r.0 + 1..r.0 + r.2 {
                let a = (y as usize * self.width + x as usize) * 3;
                let b = a - 3;
                if (0..3).all(|c| {
                    (i32::from(self.bgr[a + c]) - i32::from(self.bgr[b + c])).abs() <= SMOOTH
                }) {
                    same += 1;
                }
                total += 1;
            }
        }
        if total == 0 {
            1.0
        } else {
            same as f64 / total as f64
        }
    }
    pub fn uniformity(&self, area: RectF, glyph_height: f64) -> f64 {
        let r = self.clip(round(area));
        let margin =
            0.max((feather(glyph_height).round_ties_even() as i32).min(r.2.min(r.3) / 2 - 1));
        let (left, top, right, bottom) = (
            r.0 + margin,
            r.1 + margin,
            r.0 + r.2 - margin,
            r.1 + r.3 - margin,
        );
        if right - left < 2 || bottom - top < 2 {
            return 1.0;
        }
        let mut counts = BTreeMap::<usize, usize>::new();
        let mut total = 0;
        let step = ((f64::from(right - left) * f64::from(bottom - top) / 4096.0)
            .sqrt()
            .ceil() as usize)
            .max(1);
        for y in (top..bottom).step_by(step) {
            for x in (left..right).step_by(step) {
                let i = (y as usize * self.width + x as usize) * 3;
                let key = (usize::from(self.bgr[i + 2] >> 4) << 8)
                    | (usize::from(self.bgr[i + 1] >> 4) << 4)
                    | usize::from(self.bgr[i] >> 4);
                *counts.entry(key).or_default() += 1;
                total += 1;
            }
        }
        let mut counts: Vec<_> = counts.values().copied().collect();
        counts.sort_by(|a, b| b.cmp(a));
        if total == 0 {
            1.0
        } else {
            counts.iter().take(2).sum::<usize>() as f64 / total as f64
        }
    }
    pub fn plate(
        &self,
        area: RectF,
        wash: [u8; 3],
        text: [u8; 3],
        glyph_height: f64,
        written_height: f64,
        shape: Option<&[Vec<PointF>]>,
    ) -> Option<Plate> {
        let requested = round(area);
        if requested.2 < 2 || requested.3 < 2 {
            return None;
        }
        let rect = self.clip(requested);
        if rect.2 < 2 || rect.3 < 2 {
            return None;
        }
        let flat = if self.crispness(area, glyph_height) >= SHARP_NEIGHBOURHOOD {
            SHARP_FLAT_SHARE
        } else {
            SOFT_FLAT_SHARE
        };
        if self.uniformity(area, glyph_height) >= flat {
            return None;
        }
        let sigma = (glyph_height * BLUR_FACTOR).clamp(1.0, 16.0);
        let pad = (sigma * 3.0).ceil() as i32;
        let work = self.clip((
            rect.0 - pad,
            rect.1 - pad,
            rect.2 + pad * 2,
            rect.3 + pad * 2,
        ));
        let source = self.read(work);
        let blurred = filters::gaussian_blur(&source, sigma);
        let cropped = blurred.view(
            (rect.0 - work.0) as usize,
            (rect.1 - work.1) as usize,
            rect.2 as usize,
            rect.3 as usize,
        );
        let plate = channels::pad_replicate(
            &cropped,
            (rect.1 - requested.1) as usize,
            (requested.1 + requested.3 - rect.1 - rect.3) as usize,
            (rect.0 - requested.0) as usize,
            (requested.0 + requested.2 - rect.0 - rect.2) as usize,
        );
        if WASH_OPACITY > 0.0 {
            let tint = ImageBuffer::solid(plate.width, plate.height, [wash[2], wash[1], wash[0]]);
            arithmetic::add_weighted(&plate, 1.0 - WASH_OPACITY, &tint, WASH_OPACITY, 0.0, &plate);
        }
        let ring = feather(glyph_height).round_ties_even() as i32;
        let colors = match shape {
            None => palette(
                &plate,
                ring,
                ring.max(((plate.height as f64 - written_height) / 2.0).round_ties_even() as i32),
            ),
            Some(shape) => shape_palette(&plate, requested.0, requested.1, shape),
        };
        let mut legible = legible(&colors, text, COMFORTABLE_CONTRAST);
        if !clears(&colors, legible, [0; 3], 0.0, PLATE_CONTRAST) {
            let target = color::ensure_contrast(dominant(&colors), text, COMFORTABLE_CONTRAST);
            let lift = lift(&colors, text, target);
            let solid =
                ImageBuffer::solid(plate.width, plate.height, [target[2], target[1], target[0]]);
            arithmetic::add_weighted(&plate, 1.0 - lift, &solid, lift, 0.0, &plate);
            let changed: Vec<_> = colors.iter().map(|c| mix(*c, target, lift)).collect();
            legible = crate::screen::paint::backdrop::legible(&changed, text, COMFORTABLE_CONTRAST);
        }
        let bgra = fade(
            &plate,
            feather(glyph_height),
            requested.0,
            requested.1,
            shape,
        );
        Some(Plate {
            width: plate.width,
            height: plate.height,
            bgra,
            text: legible,
        })
    }
}
fn palette(plate: &ImageBuffer, inset_x: i32, inset_y: i32) -> Vec<[u8; 3]> {
    let mx = 0.max(inset_x.min(plate.width as i32 / 2 - 1)) as usize;
    let my = 0.max(inset_y.min(plate.height as i32 / 2 - 1)) as usize;
    let written = if mx > 0 || my > 0 {
        plate.view(mx, my, plate.width - mx * 2, plate.height - my * 2)
    } else {
        plate.isolated()
    };
    let sample = resize::to(
        &written,
        (written.width / 6).clamp(12, 48),
        (written.height / 4).clamp(6, 16),
        Interpolation::Nearest,
    );
    let mut colors = Vec::with_capacity(sample.width * sample.height);
    for y in 0..sample.height {
        for x in 0..sample.width {
            colors.push([
                sample.byte(y, x * 3 + 2),
                sample.byte(y, x * 3 + 1),
                sample.byte(y, x * 3),
            ]);
        }
    }
    colors
}
fn shape_palette(plate: &ImageBuffer, left: i32, top: i32, shape: &[Vec<PointF>]) -> Vec<[u8; 3]> {
    let step_x = (plate.width / 48).max(1);
    let step_y = (plate.height / 16).max(1);
    let mut colors = Vec::new();
    for y in (step_y / 2..plate.height).step_by(step_y) {
        for x in (step_x / 2..plate.width).step_by(step_x) {
            if !shape.iter().any(|p| {
                TiltedText::depth(
                    p,
                    f64::from(left) + x as f64 + 0.5,
                    f64::from(top) + y as f64 + 0.5,
                ) >= 0.0
            }) {
                continue;
            }
            colors.push([
                plate.byte(y, x * 3 + 2),
                plate.byte(y, x * 3 + 1),
                plate.byte(y, x * 3),
            ]);
        }
    }
    if colors.is_empty() {
        palette(plate, 0, 0)
    } else {
        colors
    }
}
fn legible(palette: &[[u8; 3]], text: [u8; 3], target: f64) -> [u8; 3] {
    let mut best = text;
    for _ in 0..4 {
        if clears(palette, best, [0; 3], 0.0, target) {
            return best;
        }
        let worst = palette
            .iter()
            .min_by(|a, b| {
                color::contrast_ratio(best, **a).total_cmp(&color::contrast_ratio(best, **b))
            })
            .unwrap();
        let next = color::ensure_contrast(best, *worst, target);
        if next == best {
            break;
        }
        best = next;
    }
    best
}
fn lift(palette: &[[u8; 3]], text: [u8; 3], target: [u8; 3]) -> f64 {
    const STEPS: usize = 16;
    for i in 1..STEPS {
        let amount = i as f64 / STEPS as f64;
        if clears(palette, text, target, amount, PLATE_CONTRAST * LIFT_MARGIN) {
            return amount;
        }
    }
    1.0
}
fn dominant(palette: &[[u8; 3]]) -> [u8; 3] {
    let mut vote = DominantColorVote::default();
    for color in palette {
        vote.add(*color);
    }
    vote.dominant().unwrap_or([0; 3])
}
fn clears(palette: &[[u8; 3]], text: [u8; 3], target: [u8; 3], amount: f64, ratio: f64) -> bool {
    palette
        .iter()
        .all(|c| color::contrast_ratio(text, mix(*c, target, amount)) >= ratio)
}
fn mix(from: [u8; 3], to: [u8; 3], amount: f64) -> [u8; 3] {
    std::array::from_fn(|c| {
        (f64::from(from[c]) + (f64::from(to[c]) - f64::from(from[c])) * amount).round_ties_even()
            as u8
    })
}
fn smoothstep(t: f64) -> u8 {
    (255.0 * t * t * (3.0 - 2.0 * t)).round_ties_even() as u8
}
fn fade(
    plate: &ImageBuffer,
    feather: f64,
    left: i32,
    top: i32,
    shape: Option<&[Vec<PointF>]>,
) -> Vec<u8> {
    let ramp = feather.round_ties_even() as usize;
    let mut pixels = vec![0; plate.width * plate.height * 4];
    for y in 0..plate.height {
        for x in 0..plate.width {
            let i = (y * plate.width + x) * 4;
            for c in 0..3 {
                pixels[i + c] = plate.byte(y, x * 3 + c);
            }
            pixels[i + 3] = if let Some(shape) = shape {
                let depth = shape
                    .iter()
                    .map(|p| {
                        TiltedText::depth(
                            p,
                            f64::from(left) + x as f64 + 0.5,
                            f64::from(top) + y as f64 + 0.5,
                        )
                    })
                    .fold(f64::NEG_INFINITY, f64::max);
                if depth >= 0.0 {
                    255
                } else if feather <= 0.0 || -depth >= feather {
                    0
                } else {
                    smoothstep((feather + depth) / feather)
                }
            } else {
                let edge = x.min(plate.width - 1 - x).min(y.min(plate.height - 1 - y));
                if ramp == 0 || edge >= ramp {
                    255
                } else {
                    smoothstep((edge as f64 + 0.5) / ramp as f64)
                }
            };
        }
    }
    pixels
}
