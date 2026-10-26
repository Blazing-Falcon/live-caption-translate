//! Shared first-run/CLI model downloads. Startup status checks sizes; downloads
//! and imported files require SHA-256 before a .part file becomes visible.

mod download;
mod manifest;

pub use download::{fetch, status, use_existing, ModelManager};
pub use manifest::{
    embedded_manifest, ArchiveMember, FileEntry, Manifest, ModelEntry, ModelState, ModelStatus,
};
