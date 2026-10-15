//! Local translation client and llama-server supervision.

pub mod client;
pub mod models;
pub mod prompts;
pub mod supervisor;
mod ternary;

pub use client::OpenAiCompatTranslator;
pub use prompts::HyMt2Prompts;

/// The caller starts the bundled server and sets server_url before construction.
pub fn register_engines(registry: &mut lt_core::engines::EngineRegistry) {
    registry
        .register_translator(|config| Ok(Box::new(OpenAiCompatTranslator::from_config(config)?)));
}
