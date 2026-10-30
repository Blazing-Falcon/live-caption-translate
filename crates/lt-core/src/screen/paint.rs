//! The colors a translation is painted with over the text it replaces.

use super::{ImageView, Rect};

/// How far outside a line the background is sampled.
const RING: i32 = 3;

/// Text over a light surface is dark and over a dark one light; luma of the stored values sits
/// near the point where both read equally well.
pub(super) fn foreground([red, green, blue]: [u8; 3]) -> [u8; 3] {
    let luma = 0.2126 * f64::from(red) + 0.7152 * f64::from(green) + 0.0722 * f64::from(blue);
    if luma > 127.5 {
        [24, 24, 24]
    } else {
        [245, 245, 245]
    }
}

/// An image and which of its pixels are text.
pub(super) struct Surface<'a> {
    image: &'a ImageView<'a>,
    text: Vec<bool>,
}

impl<'a> Surface<'a> {
    /// `lines` are the rects of every line of text in the image.
    pub(super) fn new(image: &'a ImageView<'a>, lines: &[Rect]) -> Self {
        let mut text = vec![false; image.width as usize * image.height as usize];
        for line in lines {
            for y in clamp(line.y, line.bottom(), image.height) {
                for x in clamp(line.x, line.right(), image.width) {
                    text[y * image.width as usize + x] = true;
                }
            }
        }
        Self { image, text }
    }

    /// Median color of the pixels just outside `lines` that are not text, as RGB. Falls back to
    /// the lines' own pixels when nothing surrounds them, and to white for an empty image.
    pub(super) fn background(&self, lines: &[Rect]) -> [u8; 3] {
        let mut histogram = Histogram::default();
        for line in lines {
            let (left, top) = (line.x.saturating_sub(RING), line.y.saturating_sub(RING));
            let (right, bottom) = (
                line.right().saturating_add(RING),
                line.bottom().saturating_add(RING),
            );
            for y in clamp(top, bottom, self.image.height) {
                for x in clamp(left, right, self.image.width) {
                    if !self.text[y * self.image.width as usize + x] {
                        histogram.add(self.pixel(x, y));
                    }
                }
            }
        }
        if histogram.count == 0 {
            for line in lines {
                for y in clamp(line.y, line.bottom(), self.image.height) {
                    for x in clamp(line.x, line.right(), self.image.width) {
                        histogram.add(self.pixel(x, y));
                    }
                }
            }
        }
        histogram.median().unwrap_or([255, 255, 255])
    }

    fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let at = y * self.image.stride as usize + x * 4;
        let bgra = &self.image.bgra[at..at + 3];
        [bgra[2], bgra[1], bgra[0]]
    }
}

/// The indexes in `start..end` that lie inside `0..limit`.
fn clamp(start: i32, end: i32, limit: u32) -> std::ops::Range<usize> {
    let limit = limit as i64;
    let start = i64::from(start).clamp(0, limit) as usize;
    let end = i64::from(end).clamp(0, limit) as usize;
    start..end.max(start)
}

struct Histogram {
    bins: [[u32; 256]; 3],
    count: u32,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            bins: [[0; 256]; 3],
            count: 0,
        }
    }
}

impl Histogram {
    fn add(&mut self, rgb: [u8; 3]) {
        for (bins, value) in self.bins.iter_mut().zip(rgb) {
            bins[value as usize] += 1;
        }
        self.count += 1;
    }

    fn median(&self) -> Option<[u8; 3]> {
        (self.count > 0).then(|| {
            let middle = self.count / 2;
            self.bins.map(|bins| {
                let mut seen = 0;
                for (value, &count) in bins.iter().enumerate() {
                    seen += count;
                    if seen > middle {
                        return value as u8;
                    }
                }
                255
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_ignores_text_pixels_around_a_line() {
        // Three lines of black text one pixel apart on a white page as wide as the lines: most
        // of the middle line's ring lies in the lines above and below.
        let (width, height) = (40_u32, 30_u32);
        let mut bgra = vec![255_u8; width as usize * height as usize * 4];
        let lines = [1, 10, 19].map(|y| Rect {
            x: 0,
            y,
            w: width,
            h: 8,
        });
        for line in &lines {
            for y in line.y..line.bottom() {
                for x in line.x..line.right() {
                    let at = (y as usize * width as usize + x as usize) * 4;
                    bgra[at..at + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        let image = ImageView::new(width, height, width * 4, &bgra).unwrap();
        let surface = Surface::new(&image, &lines);
        assert_eq!(surface.background(&lines[1..2]), [255, 255, 255]);
        assert_eq!(foreground([255, 255, 255]), [24, 24, 24]);
        assert_eq!(foreground([10, 10, 10]), [245, 245, 245]);
    }
}
pub mod backdrop;
pub mod color;
pub mod vote;
