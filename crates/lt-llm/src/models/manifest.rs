//! The model manifest: what the app downloads, and how a file is verified.

use lt_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const EMBEDDED_MANIFEST: &str = include_str!("../../../../models/manifest.json");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub manifest_version: u32,
    pub models: Vec<ModelEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    /// Optional models are not needed to start listening.
    #[serde(default)]
    pub optional: bool,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
    /// Non-empty: the download is a `.tar.bz2` archive. These files are taken from it by file
    /// name, checked, and kept; the archive itself is deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract: Vec<ArchiveMember>,
    /// Present: the download is a GGUF with STQ1_0 ternary weights. Its weights are rewritten
    /// as TQ2_0 into this file, which is checked and kept; the download is deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convert: Option<ArchiveMember>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchiveMember {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

impl FileEntry {
    /// The files this entry leaves in the model folder.
    pub(crate) fn local_files(&self) -> Vec<FileEntry> {
        if self.extract.is_empty() && self.convert.is_none() {
            return vec![self.clone()];
        }
        self.extract
            .iter()
            .chain(&self.convert)
            .map(|member| FileEntry {
                name: member.name.clone(),
                size: member.size,
                sha256: member.sha256.clone(),
                url: String::new(),
                extract: Vec::new(),
                convert: None,
            })
            .collect()
    }

    /// The download itself is not kept; files made from it are.
    pub(crate) fn is_source(&self) -> bool {
        !self.extract.is_empty() || self.convert.is_some()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    Missing,
    Downloading,
    Paused,
    Verifying,
    Ready,
    Corrupt,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub name: String,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub state: ModelState,
    /// Not needed to start listening.
    #[serde(default)]
    pub optional: bool,
}

pub fn embedded_manifest() -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_str(EMBEDDED_MANIFEST)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub(crate) fn validate_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.manifest_version != 1 || manifest.models.is_empty() {
        return Err(Error::Engine("Unsupported model manifest version".into()));
    }
    let mut ids = BTreeSet::new();
    for model in &manifest.models {
        if !safe_component(&model.id) || !ids.insert(&model.id) || model.files.is_empty() {
            return Err(Error::Engine(
                "Model manifest contains an unsafe or duplicate model id".into(),
            ));
        }
        let mut names = BTreeSet::new();
        for file in &model.files {
            if !safe_component(&file.name)
                || file.name.ends_with(".part")
                || !names.insert(&file.name)
            {
                return Err(Error::Engine(
                    "Model manifest contains an unsafe or duplicate filename".into(),
                ));
            }
            let valid_hash = |size: u64, sha256: &str| {
                size != 0
                    && sha256.len() == 64
                    && sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            };
            if !valid_hash(file.size, &file.sha256) {
                return Err(Error::Engine(
                    "Model manifest has an invalid size or SHA-256".into(),
                ));
            }
            if !file.extract.is_empty() && !file.name.ends_with(".tar.bz2") {
                return Err(Error::Engine(
                    "Model manifest can extract only .tar.bz2 archives".into(),
                ));
            }
            if file.convert.is_some() && (!file.extract.is_empty() || !file.name.ends_with(".gguf"))
            {
                return Err(Error::Engine(
                    "Model manifest can convert only a .gguf download".into(),
                ));
            }
            for member in file.extract.iter().chain(&file.convert) {
                if !safe_component(&member.name)
                    || member.name.ends_with(".part")
                    || !names.insert(&member.name)
                    || !valid_hash(member.size, &member.sha256)
                {
                    return Err(Error::Engine(
                        "Model manifest has an invalid archive file entry".into(),
                    ));
                }
            }
            let uri: ureq::http::Uri = file
                .url
                .parse()
                .map_err(|_| Error::Engine("Model manifest has an invalid URL".into()))?;
            if !matches!(uri.scheme_str(), Some("http" | "https"))
                || uri
                    .authority()
                    .is_none_or(|authority| authority.as_str().contains('@'))
            {
                return Err(Error::Engine(
                    "Model URL must be HTTP(S) without embedded credentials".into(),
                ));
            }
        }
    }
    Ok(())
}

fn safe_component(value: &str) -> bool {
    if value.is_empty()
        || matches!(value, "." | "..")
        || value.ends_with('.')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return false;
    }
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
