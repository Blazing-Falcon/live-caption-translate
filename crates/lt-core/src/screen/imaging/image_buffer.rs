use super::{pixel_storage::PixelStorage, saturate};
use std::sync::{atomic::Ordering, Arc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelType {
    U8C1,
    U8C3,
    F32C1,
    F32C3,
}

impl PixelType {
    pub fn channels(self) -> usize {
        if matches!(self, Self::U8C3 | Self::F32C3) {
            3
        } else {
            1
        }
    }
    pub fn is_float(self) -> bool {
        matches!(self, Self::F32C1 | Self::F32C3)
    }
    pub fn depth(self) -> usize {
        if self.is_float() {
            4
        } else {
            1
        }
    }
    pub fn pixel_size(self) -> usize {
        self.channels() * self.depth()
    }
    pub fn with_channels(self, channels: usize) -> Self {
        match (self.is_float(), channels) {
            (false, 1) => Self::U8C1,
            (false, 3) => Self::U8C3,
            (true, 1) => Self::F32C1,
            (true, 3) => Self::F32C3,
            _ => panic!("Unsupported channel count"),
        }
    }
}

pub struct ImageBuffer {
    storage: Arc<PixelStorage>,
    pub width: usize,
    pub height: usize,
    pub pixel_type: PixelType,
    pub root_width: usize,
    pub root_height: usize,
    pub origin_x: usize,
    pub origin_y: usize,
}

impl ImageBuffer {
    pub fn new(width: usize, height: usize, pixel_type: PixelType) -> Self {
        Self {
            storage: Arc::new(PixelStorage::new(
                width * height * pixel_type.channels(),
                pixel_type.is_float(),
            )),
            width,
            height,
            pixel_type,
            root_width: width,
            root_height: height,
            origin_x: 0,
            origin_y: 0,
        }
    }
    pub fn view(&self, x: usize, y: usize, width: usize, height: usize) -> Self {
        assert!(x + width <= self.width && y + height <= self.height);
        Self {
            storage: Arc::clone(&self.storage),
            width,
            height,
            pixel_type: self.pixel_type,
            root_width: self.root_width,
            root_height: self.root_height,
            origin_x: self.origin_x + x,
            origin_y: self.origin_y + y,
        }
    }
    pub fn channels(&self) -> usize {
        self.pixel_type.channels()
    }
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
    pub fn row_bytes(&self) -> usize {
        self.width * self.pixel_type.pixel_size()
    }
    pub fn stride(&self) -> usize {
        self.root_width * self.pixel_type.pixel_size()
    }
    pub fn is_continuous(&self) -> bool {
        self.stride() == self.row_bytes() || self.height <= 1
    }
    pub fn same_shape(&self, other: &Self) {
        assert_eq!(
            (self.width, self.height, self.pixel_type),
            (other.width, other.height, other.pixel_type)
        );
    }
    fn index(&self, y: usize, element: usize) -> usize {
        (self.origin_y + y) * self.root_width * self.channels()
            + self.origin_x * self.channels()
            + element
    }
    #[inline]
    pub fn byte(&self, y: usize, element: usize) -> u8 {
        match &*self.storage {
            PixelStorage::Bytes(v) => v[self.index(y, element)].load(Ordering::Relaxed),
            _ => panic!("Expected bytes"),
        }
    }
    #[inline]
    pub fn set_byte(&self, y: usize, element: usize, value: u8) {
        match &*self.storage {
            PixelStorage::Bytes(v) => v[self.index(y, element)].store(value, Ordering::Relaxed),
            _ => panic!("Expected bytes"),
        }
    }
    #[inline]
    pub fn float(&self, y: usize, element: usize) -> f32 {
        match &*self.storage {
            PixelStorage::Floats(v) => {
                f32::from_bits(v[self.index(y, element)].load(Ordering::Relaxed))
            }
            _ => panic!("Expected floats"),
        }
    }
    #[inline]
    pub fn set_float(&self, y: usize, element: usize, value: f32) {
        match &*self.storage {
            PixelStorage::Floats(v) => {
                v[self.index(y, element)].store(value.to_bits(), Ordering::Relaxed)
            }
            _ => panic!("Expected floats"),
        }
    }
    pub fn value(&self, y: usize, element: usize) -> f32 {
        if self.pixel_type.is_float() {
            self.float(y, element)
        } else {
            f32::from(self.byte(y, element))
        }
    }
    pub fn set_value(&self, y: usize, element: usize, value: f32) {
        if self.pixel_type.is_float() {
            self.set_float(y, element, value)
        } else {
            self.set_byte(y, element, saturate::to_byte(f64::from(value)))
        }
    }
    pub fn root_byte(&self, x: usize, y: usize, channel: usize) -> u8 {
        match &*self.storage {
            PixelStorage::Bytes(v) => {
                v[(y * self.root_width + x) * self.channels() + channel].load(Ordering::Relaxed)
            }
            _ => panic!("Expected bytes"),
        }
    }
    pub fn root_value(&self, x: usize, y: usize, channel: usize) -> f32 {
        let i = (y * self.root_width + x) * self.channels() + channel;
        match &*self.storage {
            PixelStorage::Bytes(v) => f32::from(v[i].load(Ordering::Relaxed)),
            PixelStorage::Floats(v) => f32::from_bits(v[i].load(Ordering::Relaxed)),
        }
    }
    pub fn isolated(&self) -> Self {
        let target = Self::new(self.width, self.height, self.pixel_type);
        self.copy_to(&target);
        target
    }
    pub fn copy_to(&self, target: &Self) {
        self.same_shape(target);
        for y in 0..self.height {
            for x in 0..self.width * self.channels() {
                if self.pixel_type.is_float() {
                    target.set_float(y, x, self.float(y, x));
                } else {
                    target.set_byte(y, x, self.byte(y, x));
                }
            }
        }
    }
    pub fn copy_masked(&self, target: &Self, mask: &Self) {
        self.same_shape(target);
        self.require_mask(mask);
        for y in 0..self.height {
            for x in 0..self.width {
                if mask.byte(y, x) != 0 {
                    for c in 0..self.channels() {
                        let i = x * self.channels() + c;
                        if self.pixel_type.is_float() {
                            target.set_float(y, i, self.float(y, i));
                        } else {
                            target.set_byte(y, i, self.byte(y, i));
                        }
                    }
                }
            }
        }
    }
    pub fn set_to(&self, value: f64, mask: Option<&Self>) {
        if let Some(mask) = mask {
            self.require_mask(mask);
        }
        for y in 0..self.height {
            for x in 0..self.width {
                if mask.is_none_or(|m| m.byte(y, x) != 0) {
                    for c in 0..self.channels() {
                        if self.pixel_type.is_float() {
                            self.set_float(y, x * self.channels() + c, value as f32)
                        } else {
                            self.set_byte(y, x * self.channels() + c, saturate::to_byte(value))
                        }
                    }
                }
            }
        }
    }
    pub fn require_mask(&self, mask: &Self) {
        assert_eq!(
            (mask.width, mask.height, mask.pixel_type),
            (self.width, self.height, PixelType::U8C1)
        );
    }
    pub fn solid(width: usize, height: usize, bgr: [u8; 3]) -> Self {
        let image = Self::new(width, height, PixelType::U8C3);
        for y in 0..height {
            for x in 0..width {
                for (c, v) in bgr.iter().enumerate() {
                    image.set_byte(y, x * 3 + c, *v);
                }
            }
        }
        image
    }
    pub fn from_floats(width: usize, height: usize, values: &[f32]) -> Self {
        assert_eq!(values.len(), width * height);
        let image = Self::new(width, height, PixelType::F32C1);
        for (i, v) in values.iter().enumerate() {
            image.set_float(i / width, i % width, *v);
        }
        image
    }
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.row_bytes() * self.height);
        for y in 0..self.height {
            for x in 0..self.width * self.channels() {
                if self.pixel_type.is_float() {
                    out.extend_from_slice(&self.float(y, x).to_le_bytes());
                } else {
                    out.push(self.byte(y, x));
                }
            }
        }
        out
    }
}
