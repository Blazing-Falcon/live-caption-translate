//! Text on a captured screen, in plain pixel buffers and rectangles.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

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
