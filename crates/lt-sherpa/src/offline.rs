//! Owned sherpa-onnx offline recognizer, stream and result handles, shared by every
//! offline model family. Results come back as the native JSON.

use crate::runtime::{
    default_runtime_path, engine_error, initialize_shared_runtime, require_file,
    verify_sherpa_runtime,
};
use lt_core::{
    engines::TokenTranscript,
    error::{Error, Result},
};
use serde::Deserialize;
use sherpa_onnx_sys as sys;
use std::{
    ffi::{c_char, CStr, CString},
    marker::PhantomData,
    path::Path,
    ptr::NonNull,
};

/// One model family's recognizer. `name` appears in error messages.
pub(crate) struct OfflineEngine {
    recognizer: Recognizer,
    name: &'static str,
    last_result_json: Option<String>,
}

impl OfflineEngine {
    /// Checks the runtime and opens every ONNX file once through ort, so a corrupt file is a
    /// Rust error instead of a native abort. The temporary sessions close before `create`.
    pub fn prepare(name: &str, runtime: Option<&Path>, models: &[&Path]) -> Result<()> {
        let runtime = match runtime {
            Some(path) => path.to_path_buf(),
            None => default_runtime_path()?,
        };
        initialize_shared_runtime(runtime)?;
        verify_sherpa_runtime(name)?;
        for model in models {
            require_file(model, name)?;
            if model.extension().is_some_and(|ext| ext == "onnx") {
                let preflight = ort::session::Session::builder()
                    .and_then(|builder| builder.with_intra_threads(1).map_err(Into::into))
                    .and_then(|builder| builder.with_inter_threads(1).map_err(Into::into))
                    .and_then(|mut builder| builder.commit_from_file(model))
                    .map_err(|error| {
                        engine_error(&format!("Cannot open the {name} model"), error)
                    })?;
                drop(preflight);
            }
        }
        Ok(())
    }

    /// `config` and every string it points to must stay valid through this call; the native
    /// constructor copies them.
    pub fn create(name: &'static str, config: &sys::OfflineRecognizerConfig) -> Result<Self> {
        // SAFETY: the caller keeps all pointers in `config` valid for this call.
        let pointer = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(config) };
        let pointer = NonNull::new(pointer.cast_mut()).ok_or_else(|| {
            Error::Engine(format!(
                "{name} could not initialize; check its model files and settings"
            ))
        })?;
        Ok(Self {
            recognizer: Recognizer(pointer),
            name,
            last_result_json: None,
        })
    }

    pub fn last_result_json(&self) -> Option<&str> {
        self.last_result_json.as_deref()
    }

    /// One recognizer run over 16 kHz mono `samples`.
    pub fn decode(&mut self, samples: &[f32]) -> Result<RawResult> {
        let name = self.name;
        if samples.is_empty() {
            return Err(Error::Engine(format!(
                "{name} received an empty audio segment"
            )));
        }
        if samples.len() > i32::MAX as usize {
            return Err(Error::Engine(format!("{name} segment is too long")));
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Engine(format!(
                "{name} received nonfinite audio samples"
            )));
        }
        let stream = self.recognizer.stream(name)?;
        stream.accept(samples);
        self.recognizer.decode(&stream);
        let result = stream.result(name)?;
        let json = result.text(name)?.to_owned();
        let raw = parse_result(&json, name)?;
        self.last_result_json = Some(json);
        Ok(raw)
    }
}

/// The C config struct with every pointer null and every number zero, which the C API
/// documents as the starting point.
pub(crate) fn zeroed_config() -> sys::OfflineRecognizerConfig {
    // SAFETY: this repr(C) config and its nested structs contain only raw pointers, integers
    // and floats; their all-zero representations are valid.
    let mut config: sys::OfflineRecognizerConfig = unsafe { std::mem::zeroed() };
    config.feat_config.sample_rate = 16_000;
    config.feat_config.feature_dim = 80;
    config
}

pub(crate) fn path_cstring(path: &Path, description: &str) -> Result<CString> {
    let path = path
        .to_str()
        .ok_or_else(|| Error::Engine(format!("{description} is not UTF-8")))?;
    CString::new(path).map_err(|_| Error::Engine(format!("{description} contains a NUL character")))
}

pub(crate) fn cstring(value: &str) -> Result<CString> {
    CString::new(value).map_err(|error| engine_error("Invalid native setting", error))
}

pub(crate) fn require_threads(name: &str, threads: u32) -> Result<()> {
    if (1..=4).contains(&threads) {
        Ok(())
    } else {
        Err(Error::Engine(format!(
            "{name} needs between one and four CPU threads"
        )))
    }
}

struct Recognizer(NonNull<sys::OfflineRecognizer>);

// SAFETY: the recognizer moves to one worker and every call requires its exclusive owning
// engine. It is never shared between threads; no Sync implementation is provided. This
// matches the official wrapper's Send usage.
unsafe impl Send for Recognizer {}

impl Recognizer {
    fn stream(&self, name: &str) -> Result<Stream<'_>> {
        // SAFETY: self owns a live recognizer; the stream lifetime cannot outlive it.
        let pointer = unsafe { sys::SherpaOnnxCreateOfflineStream(self.0.as_ptr()) };
        let pointer = NonNull::new(pointer.cast_mut())
            .ok_or_else(|| Error::Engine(format!("{name} could not create an audio stream")))?;
        Ok(Stream {
            pointer,
            recognizer: PhantomData,
        })
    }

    fn decode(&self, stream: &Stream<'_>) {
        // SAFETY: both handles are owned, live, and used on their single worker.
        unsafe { sys::SherpaOnnxDecodeOfflineStream(self.0.as_ptr(), stream.pointer.as_ptr()) }
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        // SAFETY: this uniquely owned native handle has not been destroyed.
        unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.0.as_ptr()) }
    }
}

struct Stream<'a> {
    pointer: NonNull<sys::OfflineStream>,
    recognizer: PhantomData<&'a Recognizer>,
}

impl Stream<'_> {
    fn accept(&self, samples: &[f32]) {
        // SAFETY: the caller validates the length fits i32; sample memory remains valid for
        // this synchronous call and the stream copies it.
        unsafe {
            sys::SherpaOnnxAcceptWaveformOffline(
                self.pointer.as_ptr(),
                16_000,
                samples.as_ptr(),
                samples.len() as i32,
            )
        }
    }

    fn result(&self, name: &str) -> Result<JsonResult> {
        // SAFETY: the live stream returns a separately allocated C JSON string.
        let pointer = unsafe { sys::SherpaOnnxGetOfflineStreamResultAsJson(self.pointer.as_ptr()) };
        let pointer = NonNull::new(pointer.cast_mut())
            .ok_or_else(|| Error::Engine(format!("{name} returned no recognition result")))?;
        Ok(JsonResult(pointer))
    }
}

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        // SAFETY: RAII destroys each stream exactly once before its recognizer.
        unsafe { sys::SherpaOnnxDestroyOfflineStream(self.pointer.as_ptr()) }
    }
}

struct JsonResult(NonNull<c_char>);

impl JsonResult {
    fn text(&self, name: &str) -> Result<&str> {
        // SAFETY: the C getter allocates a NUL-terminated string valid until Drop.
        unsafe { CStr::from_ptr(self.0.as_ptr()) }
            .to_str()
            .map_err(|error| engine_error(&format!("{name} result is not valid UTF-8"), error))
    }
}

impl Drop for JsonResult {
    fn drop(&mut self) {
        // SAFETY: use the matching native allocator's destroy function once.
        unsafe { sys::SherpaOnnxDestroyOfflineStreamResultJson(self.0.as_ptr()) }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
pub(crate) struct RawResult {
    pub text: String,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub tokens: Vec<String>,
    /// Token start times in seconds from the start of the audio.
    #[serde(default)]
    pub timestamps: Vec<f32>,
    /// Filled by transducer models only.
    #[serde(default)]
    pub ys_log_probs: Vec<f32>,
}

impl RawResult {
    pub fn into_tokens(self) -> TokenTranscript {
        TokenTranscript {
            text: self.text,
            tokens: self.tokens,
            timestamps: self.timestamps,
            lang_tag: self.lang,
            event: self.event,
            log_probs: self.ys_log_probs,
        }
    }
}

pub(crate) fn parse_result(json: &str, name: &str) -> Result<RawResult> {
    let mut result: RawResult = serde_json::from_str(json)
        .map_err(|error| engine_error(&format!("{name} returned invalid result JSON"), error))?;
    for field in [&mut result.lang, &mut result.event] {
        if field.as_deref().is_some_and(|text| text.trim().is_empty()) {
            *field = None;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_json_carries_tokens_with_start_times_and_log_probs() {
        let json = r#"{"lang": "<|zh|>", "emotion": "", "event": "<|Speech|>", "text": "这次 school。", "timestamps": [0.00, 0.36, 0.72, 1.10], "durations": [], "tokens":["这", "次", " school", "。"], "ys_log_probs": [-0.010000, -0.200000, -1.500000, -0.050000], "words": []}"#;
        let raw = parse_result(json, "test").unwrap();
        assert_eq!(raw.tokens, ["这", "次", " school", "。"]);
        assert_eq!(raw.timestamps, [0.0, 0.36, 0.72, 1.1]);
        assert_eq!(raw.tokens.concat(), "这次 school。");
        assert_eq!(raw.ys_log_probs.len(), 4);
        let empty = parse_result(r#"{"lang": "", "event": "", "text": ""}"#, "test").unwrap();
        assert_eq!((empty.lang, empty.event), (None, None));
    }
}
