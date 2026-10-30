use crate::screen::{ImageView, Rect};

pub struct PixelWindow<'a> {
    image: ImageView<'a>,
    pub area: Rect,
}

impl<'a> PixelWindow<'a> {
    pub fn read(image: ImageView<'a>, area: Rect) -> Option<Self> {
        (area.x >= 0
            && area.y >= 0
            && area.w > 0
            && area.h > 0
            && i64::from(area.x) + i64::from(area.w) <= i64::from(image.width)
            && i64::from(area.y) + i64::from(area.h) <= i64::from(image.height))
        .then_some(Self { image, area })
    }

    pub fn at(&self, x: i32, y: i32) -> [u8; 3] {
        let i = y as usize * self.image.stride as usize + x as usize * 4;
        [
            self.image.bgra[i + 2],
            self.image.bgra[i + 1],
            self.image.bgra[i],
        ]
    }

    pub fn distance(&self, x: i32, y: i32, rgb: [u8; 3]) -> i32 {
        distance(self.at(x, y), rgb)
    }
}

pub(super) fn distance(a: [u8; 3], b: [u8; 3]) -> i32 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (i32::from(a) - i32::from(b)).abs())
        .sum()
}

pub(super) fn contains(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

pub(super) fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    Rect {
        x: left,
        y: top,
        w: (right - left).max(0) as u32,
        h: (bottom - top).max(0) as u32,
    }
}
