//! Platform-independent Live Translation pipeline.

pub mod bus;
pub mod clock;
pub(crate) mod commit;
pub mod config;
pub mod engines;
pub mod error;
pub mod events;
#[cfg(any(test, feature = "fakes"))]
pub mod fakes;
pub mod metrics;
pub mod pipeline;
pub(crate) mod recognizer;
pub mod refine;
pub mod screen;
pub mod source;
pub mod sse;
pub(crate) mod text;
pub mod transcript;
pub mod types;
