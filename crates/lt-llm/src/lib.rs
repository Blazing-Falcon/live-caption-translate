//! Local translation client and llama-server supervision.

pub mod client;
pub mod draft;
pub mod models;
pub mod prompts;
pub mod supervisor;

pub use client::OpenAiCompatTranslator;
pub use draft::{LmtDraftTranslator, LmtPrompts};
pub use prompts::HyMt2Prompts;

/// The caller starts the bundled server and sets server_url before construction.
pub fn register_engines(registry: &mut lt_core::engines::EngineRegistry) {
    registry.register_translator("hymt2", |config| {
        Ok(Box::new(OpenAiCompatTranslator::from_config(config)?))
    });
    registry.register_draft_translator("lmt60", |config| {
        Ok(Box::new(LmtDraftTranslator::from_config(config)?))
    });
}
