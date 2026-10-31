//! Backing store for image buffers.

use std::sync::atomic::{AtomicU32, AtomicU8};

// Scoped row writers use relaxed cells; each pass joins before the next reads its output.
pub(super) enum PixelStorage {
    Bytes(Vec<AtomicU8>),
    Floats(Vec<AtomicU32>),
}

impl PixelStorage {
    pub fn new(elements: usize, floats: bool) -> Self {
        if floats {
            Self::Floats((0..elements).map(|_| AtomicU32::new(0)).collect())
        } else {
            Self::Bytes((0..elements).map(|_| AtomicU8::new(0)).collect())
        }
    }
}
