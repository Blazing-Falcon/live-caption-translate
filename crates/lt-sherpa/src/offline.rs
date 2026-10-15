//! sherpa's offline recognizer, shared by the second-pass models: one decode for each window
//! of audio.

use crate::runtime::{cstring, engine_error};
use lt_core::{
    engines::OfflineText,
    error::{Error, Result},
};
use serde::Deserialize;
use sherpa_onnx_sys as sys;
use std::{
    ffi::{c_char, CStr},
    ptr::NonNull,
};

pub(crate) struct OfflineRecognizer {
    recognizer: NonNull<sys::OfflineRecognizer>,
    name: &'static str,
}

// SAFETY: the recognizer moves to one worker with its engine, and every call takes the engine
// by `&mut self`. It is never shared between threads; no Sync implementation is provided.
unsafe impl Send for OfflineRecognizer {}

impl OfflineRecognizer {
    /// `config` and every string it points to must stay valid through this call; the native
    /// constructor copies them.
    pub(crate) fn new(name: &'static str, config: &sys::OfflineRecognizerConfig) -> Result<Self> {
        // SAFETY: the caller keeps all pointers in `config` valid for this call.
        let pointer = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(config) };
        let recognizer = NonNull::new(pointer.cast_mut()).ok_or_else(|| {
            Error::Engine(format!(
                "{name} could not initialize; check its model files"
            ))
        })?;
        Ok(Self { recognizer, name })
    }

    /// Decode 16 kHz mono `samples`. `language` sets the stream's language option, for models
    /// that read it per stream.
    pub(crate) fn decode(&self, samples: &[f32], language: Option<&str>) -> Result<OfflineText> {
        let name = self.name;
        let count = i32::try_from(samples.len())
            .map_err(|_| Error::Engine(format!("{name} audio window is too long")))?;
        let option = language
            .map(|language| Ok::<_, Error>((cstring("language")?, cstring(language)?)))
            .transpose()?;
        // SAFETY: the recognizer is live; the stream is destroyed below, before it.
        let stream = unsafe { sys::SherpaOnnxCreateOfflineStream(self.recognizer.as_ptr()) };
        let stream = NonNull::new(stream.cast_mut())
            .ok_or_else(|| Error::Engine(format!("{name} could not create an audio stream")))?;
        // SAFETY: both handles are live and used only on this worker. The stream copies the
        // option strings and the samples during the calls.
        let json = unsafe {
            if let Some((key, value)) = &option {
                sys::SherpaOnnxOfflineStreamSetOption(
                    stream.as_ptr(),
                    key.as_ptr(),
                    value.as_ptr(),
                );
            }
            sys::SherpaOnnxAcceptWaveformOffline(stream.as_ptr(), 16_000, samples.as_ptr(), count);
            sys::SherpaOnnxDecodeOfflineStream(self.recognizer.as_ptr(), stream.as_ptr());
            sys::SherpaOnnxGetOfflineStreamResultAsJson(stream.as_ptr())
        };
        let parsed = match NonNull::new(json.cast_mut()) {
            Some(json) => {
                // SAFETY: a NUL-terminated string, valid until destroyed below.
                let text = unsafe { CStr::from_ptr(json.as_ptr().cast::<c_char>()) };
                let parsed = parse_result(name, &text.to_string_lossy());
                // SAFETY: the matching destroy function, called once.
                unsafe { sys::SherpaOnnxDestroyOfflineStreamResultJson(json.as_ptr()) };
                parsed
            }
            None => Err(Error::Engine(format!("{name} returned no result"))),
        };
        // SAFETY: destroyed once, after its result.
        unsafe { sys::SherpaOnnxDestroyOfflineStream(stream.as_ptr()) };
        parsed
    }
}

impl Drop for OfflineRecognizer {
    fn drop(&mut self) {
        // SAFETY: this uniquely owned handle has not been destroyed; its streams are gone.
        unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.recognizer.as_ptr()) }
    }
}

/// The C config with every pointer null and every number zero, the C API's starting point.
pub(crate) fn zeroed_config() -> sys::OfflineRecognizerConfig {
    // SAFETY: this repr(C) config and its nested structs contain only raw pointers, integers and
    // floats; the all-zero representation is valid and selects no model family.
    let mut config: sys::OfflineRecognizerConfig = unsafe { std::mem::zeroed() };
    config.feat_config.sample_rate = 16_000;
    config
}

#[derive(Deserialize)]
struct RawResult {
    #[serde(default)]
    text: String,
    #[serde(default)]
    lang: String,
}

/// A language tag comes as `<|zh|>`; models without one leave it empty.
///
/// sherpa writes the text into its JSON without escaping control characters, and Qwen3-ASR can
/// put a line break between sentences: they become spaces, which JSON accepts between values.
fn parse_result(name: &str, json: &str) -> Result<OfflineText> {
    let json: String = json
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let raw: RawResult = serde_json::from_str(&json)
        .map_err(|error| engine_error(&format!("{name} returned invalid result JSON"), error))?;
    let lang = raw
        .lang
        .trim()
        .trim_start_matches("<|")
        .trim_end_matches("|>")
        .to_owned();
    Ok(OfflineText {
        text: raw.text.trim().to_owned(),
        lang: (!lang.is_empty()).then_some(lang),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_break_inside_the_text_does_not_fail_the_result() {
        let json =
            "{\"lang\": \"\", \"text\": \"你不信？\n你把你现在情况跟她说。\", \"tokens\": []}";
        let heard = parse_result("test", json).unwrap();
        assert_eq!(heard.text, "你不信？ 你把你现在情况跟她说。");
        assert_eq!(heard.lang, None);
        let tagged = parse_result("test", r#"{"lang": "<|zh|>", "text": "你好。"}"#).unwrap();
        assert_eq!(tagged.lang.as_deref(), Some("zh"));
    }
}
