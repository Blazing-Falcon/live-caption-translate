//! Shared first-run/CLI model downloads. Startup status checks sizes; downloads
//! and imported files require SHA-256 before a .part file becomes visible.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use lt_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const EMBEDDED_MANIFEST: &str = include_str!("../../../models/manifest.json");
// The budget covers one request including the CDN redirect and TLS start, so it must be
// long enough for useful transfer; an interrupted request resumes from the saved offset.
const BODY_BUDGET: Duration = Duration::from_secs(20);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const BUFFER_SIZE: usize = 64 * 1_024;
const NO_PROGRESS_LIMIT: usize = 4;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub manifest_version: u32,
    pub models: Vec<ModelEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
    #[serde(default)]
    pub mirror: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Huggingface,
    Modelscope,
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
}

pub fn embedded_manifest() -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_str(EMBEDDED_MANIFEST)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub struct ModelManager {
    manifest: Manifest,
    agent: ureq::Agent,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        Self::with_manifest(embedded_manifest()?)
    }

    pub fn with_manifest(manifest: Manifest) -> Result<Self> {
        // Keep ureq's default TLS and environment/system proxy discovery for downloads.
        // Body budgets deliberately terminate/retry GETs from the saved byte offset.
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_resolve(Some(Duration::from_secs(10)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_send_request(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            .timeout_recv_body(Some(BODY_BUDGET))
            .max_idle_connections(0)
            .build()
            .into();
        Self::with_manifest_and_agent(manifest, agent)
    }

    /// Test/embedding seam; supplied agents must retain a bounded body budget.
    pub fn with_manifest_and_agent(manifest: Manifest, agent: ureq::Agent) -> Result<Self> {
        validate_manifest(&manifest)?;
        Ok(Self { manifest, agent })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn source_supported(&self, source: ModelSource) -> bool {
        source == ModelSource::Huggingface
            || self
                .manifest
                .models
                .iter()
                .flat_map(|model| &model.files)
                .all(|file| !file.mirror.is_empty())
    }

    pub fn status(&self, dir: impl AsRef<Path>) -> Result<Vec<ModelStatus>> {
        self.manifest
            .models
            .iter()
            .map(|model| model_status(dir.as_ref(), model))
            .collect()
    }

    pub fn fetch(
        &self,
        source: ModelSource,
        dir: impl AsRef<Path>,
        cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(ModelStatus),
    ) -> Result<Vec<ModelStatus>> {
        if !self.source_supported(source) {
            return Err(Error::Engine(
                "These exact model files have no verified ModelScope mirror. Try Hugging Face."
                    .into(),
            ));
        }
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;
        let mut progress = Progress::new(on_progress);
        let mut files: Vec<_> = self
            .manifest
            .models
            .iter()
            .enumerate()
            .flat_map(|(model_index, model)| {
                model
                    .files
                    .iter()
                    .enumerate()
                    .map(move |(file_index, file)| (file.size, model_index, file_index))
            })
            .collect();
        files.sort_by_key(|entry| entry.0);
        for (_, model_index, file_index) in files {
            let model = &self.manifest.models[model_index];
            let file = &model.files[file_index];
            let paths = file_paths(dir, model, file, true)?;
            check_cancel(cancelled, &mut progress, dir, model)?;
            if regular_len(&paths.final_file)?.is_some() {
                progress.emit(dir, model, ModelState::Verifying, true)?;
                if verified(
                    &paths.final_file,
                    file,
                    cancelled,
                    &mut progress,
                    dir,
                    model,
                )? {
                    progress.complete(dir, model)?;
                    continue;
                }
                progress.emit(dir, model, ModelState::Corrupt, true)?;
                remove_regular(&paths.final_file)?;
            }
            let url = match source {
                ModelSource::Huggingface => &file.url,
                ModelSource::Modelscope => &file.mirror,
            };
            let mut matched = false;
            for attempt in 0..2 {
                self.download(
                    DownloadFile {
                        url,
                        file,
                        partial: &paths.partial,
                        dir,
                        model,
                    },
                    cancelled,
                    &mut progress,
                )?;
                progress.emit(dir, model, ModelState::Verifying, true)?;
                if verified(&paths.partial, file, cancelled, &mut progress, dir, model)? {
                    matched = true;
                    break;
                }
                progress.emit(dir, model, ModelState::Corrupt, true)?;
                remove_regular(&paths.partial)?;
                if attempt == 1 {
                    break;
                }
            }
            if !matched {
                let message = if source == ModelSource::Modelscope {
                    "Mirror copy does not match. Try Hugging Face.".into()
                } else {
                    format!("Downloaded model failed its SHA-256 check: {}", file.name)
                };
                return Err(Error::Engine(message));
            }
            check_cancel(cancelled, &mut progress, dir, model)?;
            promote(&paths.partial, &paths.final_file)?;
            progress.complete(dir, model)?;
        }
        self.status(dir)
    }

    pub fn use_existing(
        &self,
        from: impl AsRef<Path>,
        dir: impl AsRef<Path>,
        cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(ModelStatus),
    ) -> Result<Vec<ModelStatus>> {
        let from = from.as_ref();
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;
        let mut progress = Progress::new(on_progress);
        let mut found = false;
        for model in &self.manifest.models {
            for file in &model.files {
                check_cancel(cancelled, &mut progress, dir, model)?;
                let flat = from.join(&file.name);
                let nested = from.join(&model.id).join(&file.name);
                let source = if regular_len(&flat)?.is_some() {
                    flat
                } else if regular_len(&nested)?.is_some() {
                    nested
                } else {
                    continue;
                };
                found = true;
                progress.emit(dir, model, ModelState::Verifying, true)?;
                if !verified(&source, file, cancelled, &mut progress, dir, model)? {
                    return Err(Error::Engine(format!(
                        "Existing model does not match its expected SHA-256: {}",
                        file.name
                    )));
                }
                let paths = file_paths(dir, model, file, true)?;
                if source.canonicalize()?
                    == paths
                        .final_file
                        .canonicalize()
                        .unwrap_or_else(|_| paths.final_file.clone())
                {
                    progress.complete(dir, model)?;
                    continue;
                }
                progress.emit(dir, model, ModelState::Downloading, true)?;
                let mut input = File::open(&source)?;
                let mut output = open_partial(&paths.partial, false)?;
                let mut buffer = [0_u8; BUFFER_SIZE];
                loop {
                    check_cancel(cancelled, &mut progress, dir, model)?;
                    let read = input.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    output.write_all(&buffer[..read])?;
                    progress.emit(dir, model, ModelState::Downloading, false)?;
                }
                output.sync_all()?;
                drop(output);
                progress.emit(dir, model, ModelState::Verifying, true)?;
                if !verified(&paths.partial, file, cancelled, &mut progress, dir, model)? {
                    return Err(Error::Engine(format!(
                        "Model changed while being copied: {}",
                        file.name
                    )));
                }
                check_cancel(cancelled, &mut progress, dir, model)?;
                promote(&paths.partial, &paths.final_file)?;
                progress.complete(dir, model)?;
            }
        }
        if !found {
            return Err(Error::Engine(
                "No matching model filenames were found in that folder".into(),
            ));
        }
        self.status(dir)
    }

    fn download(
        &self,
        job: DownloadFile<'_>,
        cancelled: &AtomicBool,
        progress: &mut Progress<'_>,
    ) -> Result<()> {
        let DownloadFile {
            url,
            file,
            partial,
            dir,
            model,
        } = job;
        if regular_len(partial)?.is_some_and(|size| size > file.size) {
            remove_regular(partial)?;
        }
        let mut no_progress = 0;
        let mut ignored_ranges = 0;
        loop {
            check_cancel(cancelled, progress, dir, model)?;
            let offset = regular_len(partial)?.unwrap_or(0);
            if offset == file.size {
                return Ok(());
            }
            progress.emit(dir, model, ModelState::Downloading, false)?;
            check_cancel(cancelled, progress, dir, model)?;
            let mut request = self.agent.get(url).header("Accept-Encoding", "identity");
            if offset != 0 {
                request = request.header("Range", format!("bytes={offset}-"));
            }
            let mut response = match request.call() {
                Ok(response) => response,
                Err(error) => {
                    tracing::debug!(%error, "Model download request failed");
                    no_progress += 1;
                    if no_progress >= NO_PROGRESS_LIMIT {
                        return Err(Error::Engine(format!(
                            "Could not connect to the model download server for {}",
                            file.name
                        )));
                    }
                    retry_delay(cancelled)?;
                    continue;
                }
            };
            if response
                .headers()
                .get("content-encoding")
                .is_some_and(|value| value.to_str().map_or(true, |value| value != "identity"))
            {
                return Err(Error::Engine(
                    "Model download returned encoded bytes instead of the requested file".into(),
                ));
            }
            let (start, body_len, append) = match response.status().as_u16() {
                200 => {
                    if offset != 0 {
                        ignored_ranges += 1;
                        if ignored_ranges > 3 {
                            return Err(Error::Engine(
                                "Download server repeatedly ignored resume requests".into(),
                            ));
                        }
                    }
                    (0, file.size, false)
                }
                206 => {
                    let value = response
                        .headers()
                        .get("content-range")
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            Error::Engine("Resume response is missing Content-Range".into())
                        })?;
                    let range = content_range(value).ok_or_else(|| {
                        Error::Engine("Resume response has an invalid Content-Range".into())
                    })?;
                    if range.0 != offset || range.2 != file.size {
                        return Err(Error::Engine(
                            "Resume response does not match the requested file offset/size".into(),
                        ));
                    }
                    (offset, range.1 - range.0 + 1, true)
                }
                416 => {
                    let expected = format!("bytes */{}", file.size);
                    if offset == file.size
                        && response
                            .headers()
                            .get("content-range")
                            .and_then(|v| v.to_str().ok())
                            == Some(expected.as_str())
                    {
                        return Ok(());
                    }
                    return Err(Error::Engine(
                        "Download server rejected an incomplete resume request".into(),
                    ));
                }
                status => {
                    return Err(Error::Engine(format!(
                        "Model download server returned HTTP {status}"
                    )))
                }
            };
            if let Some(length) = response.headers().get("content-length") {
                if length
                    .to_str()
                    .ok()
                    .and_then(|length| length.parse::<u64>().ok())
                    != Some(body_len)
                {
                    return Err(Error::Engine(
                        "Model response length does not match its declared range/size".into(),
                    ));
                }
            }
            let mut output = open_partial(partial, append)?;
            let mut reader = response.body_mut().as_reader();
            let mut buffer = [0_u8; BUFFER_SIZE];
            let mut written = 0_u64;
            let mut interrupted = false;
            while written < body_len {
                check_cancel(cancelled, progress, dir, model)?;
                let limit = (body_len - written).min(BUFFER_SIZE as u64) as usize;
                match reader.read(&mut buffer[..limit]) {
                    Ok(0) => {
                        interrupted = true;
                        break;
                    }
                    Ok(read) => {
                        output.write_all(&buffer[..read])?;
                        written += read as u64;
                        progress.emit(dir, model, ModelState::Downloading, false)?;
                    }
                    Err(error) => {
                        tracing::debug!(%error, offset, written, "Model download read interrupted");
                        interrupted = true;
                        break;
                    }
                }
            }
            output.sync_all()?;
            drop(output);
            if written == body_len {
                // Reject extra bytes even for chunked responses; EOF validates the framing.
                let mut extra = [0_u8; 1];
                if reader.read(&mut extra).is_ok_and(|read| read != 0) {
                    return Err(Error::Engine(
                        "Model response exceeded its declared file/range size".into(),
                    ));
                }
            }
            let after = regular_len(partial)?.unwrap_or(0);
            if after == file.size {
                return Ok(());
            }
            if after <= offset || (interrupted && written == 0) {
                no_progress += 1;
            } else {
                no_progress = 0;
            }
            if no_progress >= NO_PROGRESS_LIMIT {
                return Err(Error::Engine(format!(
                    "Model download stopped making progress: {}",
                    file.name
                )));
            }
            if after < start + written {
                return Err(Error::Engine(
                    "Partial model length changed during download".into(),
                ));
            }
            retry_delay(cancelled)?;
        }
    }
}

pub fn status(dir: impl AsRef<Path>) -> Result<Vec<ModelStatus>> {
    ModelManager::new()?.status(dir)
}
pub fn fetch(
    source: ModelSource,
    dir: impl AsRef<Path>,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(ModelStatus),
) -> Result<Vec<ModelStatus>> {
    ModelManager::new()?.fetch(source, dir, cancelled, on_progress)
}
pub fn use_existing(
    from: impl AsRef<Path>,
    dir: impl AsRef<Path>,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(ModelStatus),
) -> Result<Vec<ModelStatus>> {
    ModelManager::new()?.use_existing(from, dir, cancelled, on_progress)
}

struct FilePaths {
    final_file: PathBuf,
    partial: PathBuf,
}

struct DownloadFile<'a> {
    url: &'a str,
    file: &'a FileEntry,
    partial: &'a Path,
    dir: &'a Path,
    model: &'a ModelEntry,
}

fn file_paths(dir: &Path, model: &ModelEntry, file: &FileEntry, create: bool) -> Result<FilePaths> {
    let folder = dir.join(&model.id);
    if create {
        fs::create_dir_all(&folder)?;
    }
    if folder.exists() && !folder.canonicalize()?.starts_with(dir.canonicalize()?) {
        return Err(Error::Engine(
            "Model folder escapes the selected model directory".into(),
        ));
    }
    let paths = FilePaths {
        final_file: folder.join(&file.name),
        partial: folder.join(format!("{}.part", file.name)),
    };
    regular_len(&paths.final_file)?;
    regular_len(&paths.partial)?;
    Ok(paths)
}

fn regular_len(path: &Path) -> Result<Option<u64>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(Error::Engine(
            "Model path must be a regular file, not a link or directory".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(error)),
    }
}

fn model_status(dir: &Path, model: &ModelEntry) -> Result<ModelStatus> {
    let mut total = 0_u64;
    let mut done = 0_u64;
    let mut ready = true;
    let mut corrupt = false;
    let mut paused = false;
    for file in &model.files {
        total = total.saturating_add(file.size);
        let paths = file_paths(dir, model, file, false)?;
        match regular_len(&paths.final_file)? {
            Some(size) if size == file.size => {
                done = done.saturating_add(size);
            }
            Some(_) => {
                ready = false;
                corrupt = true;
            }
            None => {
                ready = false;
            }
        }
        if regular_len(&paths.final_file)? != Some(file.size) {
            if let Some(size) = regular_len(&paths.partial)? {
                done = done.saturating_add(size.min(file.size));
                paused |= size != 0;
            }
        }
    }
    let state = if ready {
        ModelState::Ready
    } else if corrupt {
        ModelState::Corrupt
    } else if paused {
        ModelState::Paused
    } else {
        ModelState::Missing
    };
    Ok(ModelStatus {
        id: model.id.clone(),
        name: model.name.clone(),
        bytes_total: total,
        bytes_done: done,
        state,
    })
}

fn verified(
    path: &Path,
    file: &FileEntry,
    cancelled: &AtomicBool,
    progress: &mut Progress<'_>,
    dir: &Path,
    model: &ModelEntry,
) -> Result<bool> {
    if regular_len(path)? != Some(file.size) {
        return Ok(false);
    }
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        check_cancel(cancelled, progress, dir, model)?;
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        progress.emit(dir, model, ModelState::Verifying, false)?;
    }
    Ok(format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&file.sha256))
}

fn open_partial(path: &Path, append: bool) -> Result<File> {
    regular_len(path)?;
    Ok(OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)?)
}

fn remove_regular(path: &Path) -> Result<()> {
    if regular_len(path)?.is_some() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn promote(partial: &Path, final_file: &Path) -> Result<()> {
    // An existing destination here is known to be corrupt. The verified .part
    // remains hidden until this same-directory rename; no unverified final exists.
    remove_regular(final_file)?;
    fs::rename(partial, final_file)?;
    Ok(())
}

fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (span, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    let total = total.parse::<u64>().ok()?;
    (start <= end && end < total).then_some((start, end, total))
}

fn check_cancel(
    cancelled: &AtomicBool,
    progress: &mut Progress<'_>,
    dir: &Path,
    model: &ModelEntry,
) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        progress.emit(dir, model, ModelState::Paused, true)?;
        return Err(Error::Stopped);
    }
    Ok(())
}

fn retry_delay(cancelled: &AtomicBool) -> Result<()> {
    for _ in 0..4 {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

struct Progress<'a> {
    callback: &'a mut dyn FnMut(ModelStatus),
    last: Option<Instant>,
    state: Option<(String, ModelState)>,
}

impl<'a> Progress<'a> {
    fn new(callback: &'a mut dyn FnMut(ModelStatus)) -> Self {
        Self {
            callback,
            last: None,
            state: None,
        }
    }
    fn emit(
        &mut self,
        dir: &Path,
        model: &ModelEntry,
        state: ModelState,
        force: bool,
    ) -> Result<()> {
        let changed = self
            .state
            .as_ref()
            .is_none_or(|previous| previous.0 != model.id || previous.1 != state);
        if force
            || changed
            || self
                .last
                .is_none_or(|last| last.elapsed() >= PROGRESS_INTERVAL)
        {
            let mut status = model_status(dir, model)?;
            status.state = state;
            (self.callback)(status);
            self.last = Some(Instant::now());
            self.state = Some((model.id.clone(), state));
        }
        Ok(())
    }
    fn complete(&mut self, dir: &Path, model: &ModelEntry) -> Result<()> {
        let status = model_status(dir, model)?;
        self.emit(
            dir,
            model,
            if status.state == ModelState::Ready {
                ModelState::Ready
            } else {
                ModelState::Downloading
            },
            true,
        )
    }
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
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
            if file.size == 0
                || file.sha256.len() != 64
                || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(Error::Engine(
                    "Model manifest has an invalid size or SHA-256".into(),
                ));
            }
            for (index, url) in [&file.url, &file.mirror].into_iter().enumerate() {
                if url.is_empty() && index == 1 {
                    continue;
                }
                let uri: ureq::http::Uri = url
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_manifest_is_valid_and_sizes_are_known() {
        let manifest = embedded_manifest().unwrap();
        assert_eq!(manifest.models.len(), 3);
        assert_eq!(
            manifest
                .models
                .iter()
                .flat_map(|model| &model.files)
                .count(),
            4
        );
        assert!(!ModelManager::new()
            .unwrap()
            .source_supported(ModelSource::Modelscope));
    }
    #[test]
    fn ranges_and_portable_components_are_strict() {
        assert_eq!(content_range("bytes 10-19/20"), Some((10, 19, 20)));
        for range in [
            "bytes 20-19/20",
            "bytes 0-20/20",
            "bytes */20",
            "bytes 0-1/*",
            "items 0-1/20",
        ] {
            assert!(content_range(range).is_none());
        }
        for path in [
            "../bad", "a/b", "a\\b", "C:bad", "..", ".", "CON", "nul.bin", "COM1.txt", "bad.",
        ] {
            assert!(!safe_component(path));
        }
    }
}
