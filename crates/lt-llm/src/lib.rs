//! Local translation client and llama-server supervision.

pub mod client;
pub mod models;
pub mod prompts;
pub mod supervisor;
mod ternary;

pub use client::{OpenAiCompatTranslator, LEAKED_MARKERS};
pub use prompts::HyMt2Prompts;
