//! Owned C-API recognizer/stream/result handles retain SenseVoice metadata.

use crate::runtime::{
    default_runtime_path, engine_error, initialize_shared_runtime, require_file,
    verify_sensevoice_runtime,
};
use lt_core::{
    config::AsrConfig,
    engines::SegmentAsr,
    error::{Error, Result},
    text::{classify_with_lang, clean},
    types::{Segment, StageTiming, TextClass, Transcript},
};
use serde::Deserialize;
use sherpa_onnx_sys as sys;
use std::{
    ffi::{c_char, CStr, CString},
    marker::PhantomData,
    path::Path,
    ptr::NonNull,
    time::Instant,
};

pub struct SenseVoiceAsr {
    recognizer: Recognizer,
    last_result_json: Option<String>,
}

impl SenseVoiceAsr {
    pub fn new(
        model: impl AsRef<Path>,
        tokens: impl AsRef<Path>,
        settings: &AsrConfig,
    ) -> Result<Self> {
        Self::with_runtime(model, tokens, settings, default_runtime_path()?)
    }

    pub fn with_runtime(
        model: impl AsRef<Path>,
        tokens: impl AsRef<Path>,
        settings: &AsrConfig,
        runtime: impl AsRef<Path>,
    ) -> Result<Self> {
        let model = model.as_ref();
        let tokens = tokens.as_ref();
        require_file(model, "SenseVoice 2024 model")?;
        require_file(tokens, "SenseVoice token table")?;
        if !(1..=4).contains(&settings.threads) {
            return Err(Error::Engine(
                "SenseVoice needs between one and four CPU threads".into(),
            ));
        }
        initialize_shared_runtime(runtime)?;
        verify_sensevoice_runtime()?;
        // Validate ONNX loading in a fallible Rust API before the C++ constructor
        // can encounter corrupt model bytes. The temporary session drops before
        // creating the recognizer so there is only one resident ASR model.
        let preflight = ort::session::Session::builder()
            .and_then(|builder| builder.with_intra_threads(1).map_err(Into::into))
            .and_then(|builder| builder.with_inter_threads(1).map_err(Into::into))
            .and_then(|mut builder| builder.commit_from_file(model))
            .map_err(|error| engine_error("Cannot open the SenseVoice 2024 model", error))?;
        drop(preflight);
        let model = path_cstring(model, "SenseVoice model path")?;
        let tokens = path_cstring(tokens, "SenseVoice tokens path")?;
        let language = CString::new(settings.language.as_str())
            .map_err(|_| Error::Engine("SenseVoice language contains a NUL character".into()))?;
        let provider =
            CString::new("cpu").map_err(|error| engine_error("Invalid CPU provider", error))?;
        let decoding = CString::new("greedy_search")
            .map_err(|error| engine_error("Invalid decoding method", error))?;
        // SAFETY: this repr(C) config and its nested structs contain only raw
        // pointers, integers and floats; their all-zero representations are
        // valid. Unselected model-family paths remain null as in the C examples.
        let mut config: sys::OfflineRecognizerConfig = unsafe { std::mem::zeroed() };
        config.feat_config.sample_rate = 16_000;
        config.feat_config.feature_dim = 80;
        config.model_config.tokens = tokens.as_ptr();
        config.model_config.num_threads = settings.threads as i32;
        config.model_config.provider = provider.as_ptr();
        config.model_config.sense_voice.model = model.as_ptr();
        config.model_config.sense_voice.language = language.as_ptr();
        config.model_config.sense_voice.use_itn = i32::from(settings.use_itn);
        config.decoding_method = decoding.as_ptr();
        config.max_active_paths = 4;
        // SAFETY: all pointers above are valid CStrings through this call. The
        // native constructor copies configuration strings into owned std::strings.
        let pointer = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(&config) };
        let pointer = NonNull::new(pointer.cast_mut()).ok_or_else(|| Error::Engine("SenseVoice could not initialize; check the 2024 model, tokens and language settings".into()))?;
        Ok(Self {
            recognizer: Recognizer(pointer),
            last_result_json: None,
        })
    }

    /// Native JSON is retained for explicit local metadata diagnostics and
    /// parity tests. It is never logged automatically.
    pub fn last_result_json(&self) -> Option<&str> {
        self.last_result_json.as_deref()
    }
}

impl SegmentAsr for SenseVoiceAsr {
    fn transcribe(&mut self, segment: &Segment) -> Result<Transcript> {
        if segment.samples.is_empty() {
            return Err(Error::Engine(
                "SenseVoice received an empty audio segment".into(),
            ));
        }
        if segment.samples.len() > i32::MAX as usize {
            return Err(Error::Engine("SenseVoice segment is too long".into()));
        }
        if segment.samples.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Engine(
                "SenseVoice received nonfinite audio samples".into(),
            ));
        }
        let started = Instant::now();
        let stream = self.recognizer.stream()?;
        stream.accept(&segment.samples);
        self.recognizer.decode(&stream);
        let result = stream.result()?;
        let json = result.text()?.to_owned();
        let raw = parse_result(&json)?;
        self.last_result_json = Some(json);
        let clean_text = clean(&raw.text, &[]);
        let class =
            classify_with_lang(&clean_text, raw.lang.as_deref()).unwrap_or(TextClass::Chinese);
        Ok(Transcript {
            id: segment.id,
            text: raw.text,
            lang_tag: raw.lang,
            class,
            event: raw.event,
            timing: StageTiming {
                start: segment.start,
                end: segment.end,
                asr_done_ms: 0,
                asr_ms: started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
            },
            absorbed: Vec::new(),
        })
    }
}

struct Recognizer(NonNull<sys::OfflineRecognizer>);

// SAFETY: the recognizer moves to one ASR worker and every call requires its
// exclusive owning SenseVoiceAsr. It is never shared between threads; no Sync
// implementation is provided. This matches the official wrapper's Send usage.
unsafe impl Send for Recognizer {}

impl Recognizer {
    fn stream(&self) -> Result<Stream<'_>> {
        // SAFETY: self owns a live recognizer; the stream lifetime cannot outlive it.
        let pointer = unsafe { sys::SherpaOnnxCreateOfflineStream(self.0.as_ptr()) };
        let pointer = NonNull::new(pointer.cast_mut())
            .ok_or_else(|| Error::Engine("SenseVoice could not create an audio stream".into()))?;
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
        // SAFETY: the parent validates the length fits i32; sample memory remains
        // valid for this synchronous call and the stream copies/features it.
        unsafe {
            sys::SherpaOnnxAcceptWaveformOffline(
                self.pointer.as_ptr(),
                16_000,
                samples.as_ptr(),
                samples.len() as i32,
            )
        }
    }

    fn result(&self) -> Result<JsonResult> {
        // SAFETY: the live stream returns a separately allocated C JSON string.
        let pointer = unsafe { sys::SherpaOnnxGetOfflineStreamResultAsJson(self.pointer.as_ptr()) };
        let pointer = NonNull::new(pointer.cast_mut())
            .ok_or_else(|| Error::Engine("SenseVoice returned no recognition result".into()))?;
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
    fn text(&self) -> Result<&str> {
        // SAFETY: the C getter allocates a NUL-terminated string valid until Drop.
        unsafe { CStr::from_ptr(self.0.as_ptr()) }
            .to_str()
            .map_err(|error| engine_error("SenseVoice result is not valid UTF-8", error))
    }
}

impl Drop for JsonResult {
    fn drop(&mut self) {
        // SAFETY: use the matching native allocator's destroy function once.
        unsafe { sys::SherpaOnnxDestroyOfflineStreamResultJson(self.0.as_ptr()) }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
struct RawResult {
    text: String,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    event: Option<String>,
}

fn parse_result(json: &str) -> Result<RawResult> {
    let mut result: RawResult = serde_json::from_str(json)
        .map_err(|error| engine_error("SenseVoice returned invalid result JSON", error))?;
    for field in [&mut result.lang, &mut result.event] {
        if field.as_deref().is_some_and(|text| text.trim().is_empty()) {
            *field = None;
        }
    }
    Ok(result)
}

fn path_cstring(path: &Path, description: &str) -> Result<CString> {
    let path = path
        .to_str()
        .ok_or_else(|| Error::Engine(format!("{description} is not UTF-8")))?;
    CString::new(path).map_err(|_| Error::Engine(format!("{description} contains a NUL character")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_json_preserves_language_event_and_ignores_extra_metadata() {
        let cases = [
            (
                r#"{"text":"你好。","lang":"<|zh|>","event":"<|Speech|>","emotion":"<|NEUTRAL|>","tokens":[],"timestamps":[]}"#,
                "你好。",
                Some("<|zh|>"),
                Some("<|Speech|>"),
            ),
            (
                r#"{"text":"啦。","lang":"<|ko|>","event":"<|BGM|>"}"#,
                "啦。",
                Some("<|ko|>"),
                Some("<|BGM|>"),
            ),
            (r#"{"text":"","lang":"","event":""}"#, "", None, None),
            (r#"{"text":"Hello."}"#, "Hello.", None, None),
        ];
        for (json, text, lang, event) in cases {
            assert_eq!(
                parse_result(json).unwrap(),
                RawResult {
                    text: text.into(),
                    lang: lang.map(String::from),
                    event: event.map(String::from)
                }
            );
        }
        assert!(parse_result("{}").is_err());
        assert!(parse_result("not JSON").is_err());
    }

    #[test]
    fn engine_types_can_move_to_their_exclusive_workers() {
        fn assert_send<T: Send>() {}
        assert_send::<SenseVoiceAsr>();
        assert_send::<crate::SileroVad>();
    }
}
