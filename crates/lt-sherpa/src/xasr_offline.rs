//! Offline X-ASR: the same zipformer transducer as the live recognizer, decoded over a whole
//! window with full attention. It reads each closed clause again with the speech before it.

use crate::runtime::{cstring, engine_error, path_cstring, prepare};
use lt_core::{
    config::AsrConfig,
    engines::{OfflineAsr, OfflineText},
    error::{Error, Result},
};
use serde::Deserialize;
use sherpa_onnx_sys as sys;
use std::{
    ffi::{c_char, CStr},
    path::Path,
    ptr::NonNull,
};

const NAME: &str = "X-ASR offline";
/// Earlier speech read with each window. A clause of a few words alone loses its commas and
/// guesses words its context decides; much more context makes the decode slower and moves the
/// best match away from the clause.
const CONTEXT_S: f64 = 5.0;
/// The encoder's first convolutions reject fewer than about 0.1 s of audio with a C++ exception,
/// which would abort the process; a shorter window (a clause right after a break) is padded with
/// silence to this length.
const MIN_SAMPLES: usize = 8_000;

pub struct XAsrOffline {
    recognizer: NonNull<sys::OfflineRecognizer>,
}

// SAFETY: the recognizer moves to the refine worker and every call takes `&mut self`. It is never
// shared between threads; no Sync implementation is provided.
unsafe impl Send for XAsrOffline {}

impl XAsrOffline {
    /// `dir` holds the encoder, decoder, joiner and tokens of the manifest entry.
    pub fn new(dir: impl AsRef<Path>, settings: &AsrConfig) -> Result<Self> {
        let dir = dir.as_ref();
        let encoder = dir.join("encoder-epoch-99-avg-1.int8.onnx");
        let decoder = dir.join("decoder-epoch-99-avg-1.onnx");
        let joiner = dir.join("joiner-epoch-99-avg-1.int8.onnx");
        let tokens = dir.join("tokens.txt");
        prepare(NAME, None, &[&encoder, &decoder, &joiner, &tokens])?;
        let encoder = path_cstring(&encoder, "X-ASR offline encoder path")?;
        let decoder = path_cstring(&decoder, "X-ASR offline decoder path")?;
        let joiner = path_cstring(&joiner, "X-ASR offline joiner path")?;
        let tokens = path_cstring(&tokens, "X-ASR offline tokens path")?;
        let provider = cstring("cpu")?;
        let decoding = cstring("greedy_search")?;
        // SAFETY: this repr(C) config contains only raw pointers, integers and floats; the
        // all-zero representation is valid and selects no other model family.
        let mut config: sys::OfflineRecognizerConfig = unsafe { std::mem::zeroed() };
        config.feat_config.sample_rate = 16_000;
        config.feat_config.feature_dim = 80;
        config.model_config.transducer.encoder = encoder.as_ptr();
        config.model_config.transducer.decoder = decoder.as_ptr();
        config.model_config.transducer.joiner = joiner.as_ptr();
        config.model_config.tokens = tokens.as_ptr();
        config.model_config.num_threads = settings.second_pass_engine_threads() as i32;
        config.model_config.provider = provider.as_ptr();
        config.decoding_method = decoding.as_ptr();
        config.max_active_paths = 4;
        // SAFETY: every pointer above is a live CString through this call; the native
        // constructor copies the strings.
        let pointer = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(&config) };
        let recognizer = NonNull::new(pointer.cast_mut()).ok_or_else(|| {
            Error::Engine(format!(
                "{NAME} could not initialize; check its model files"
            ))
        })?;
        Ok(Self { recognizer })
    }
}

#[derive(Deserialize)]
struct OfflineResult {
    #[serde(default)]
    text: String,
}

impl OfflineAsr for XAsrOffline {
    fn decode(&mut self, samples: &[f32], _language: Option<&str>) -> Result<OfflineText> {
        if samples.len() > i32::MAX as usize {
            return Err(Error::Engine(format!("{NAME} audio window is too long")));
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Engine(format!(
                "{NAME} received nonfinite audio samples"
            )));
        }
        let padded;
        let samples = if samples.len() < MIN_SAMPLES {
            let mut copy = samples.to_vec();
            copy.resize(MIN_SAMPLES, 0.0);
            padded = copy;
            &padded[..]
        } else {
            samples
        };
        // SAFETY: the recognizer is live; the stream is destroyed below on every path.
        let pointer = unsafe { sys::SherpaOnnxCreateOfflineStream(self.recognizer.as_ptr()) };
        let stream = NonNull::new(pointer.cast_mut())
            .ok_or_else(|| Error::Engine(format!("{NAME} could not create an audio stream")))?;
        // SAFETY: both handles are live and used only on this worker; the stream copies the
        // samples during the call, and the JSON string is freed by its destroy function.
        let text = unsafe {
            sys::SherpaOnnxAcceptWaveformOffline(
                stream.as_ptr(),
                16_000,
                samples.as_ptr(),
                samples.len() as i32,
            );
            sys::SherpaOnnxDecodeOfflineStream(self.recognizer.as_ptr(), stream.as_ptr());
            let json = sys::SherpaOnnxGetOfflineStreamResultAsJson(stream.as_ptr());
            let text = if json.is_null() {
                Err(Error::Engine(format!("{NAME} returned no result")))
            } else {
                let parsed = parse_text(&CStr::from_ptr(json.cast::<c_char>()).to_string_lossy());
                sys::SherpaOnnxDestroyOfflineStreamResultJson(json);
                parsed
            };
            sys::SherpaOnnxDestroyOfflineStream(stream.as_ptr());
            text
        }?;
        Ok(OfflineText { text, lang: None })
    }

    fn context_s(&self) -> f64 {
        CONTEXT_S
    }
}

fn parse_text(json: &str) -> Result<String> {
    let raw: OfflineResult = serde_json::from_str(&json.replace('\u{FFFD}', ""))
        .map_err(|error| engine_error(&format!("{NAME} returned invalid result JSON"), error))?;
    Ok(tidy(&raw.text))
}

/// sherpa puts a space after every mark and around English words. Keep it where the live text
/// has one (`normalize_tokens`): before an English word, and between an English word and the
/// Chinese after it, so the final text is spelled like the live text.
fn tidy(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out = String::with_capacity(text.len());
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            let before = words[index - 1].chars().next_back().unwrap_or(' ');
            let after = word.chars().next().unwrap_or(' ');
            if after.is_ascii_alphanumeric()
                || (before.is_ascii_alphanumeric() && after.is_alphanumeric())
            {
                out.push(' ');
            }
        }
        out.push_str(word);
    }
    out
}

impl Drop for XAsrOffline {
    fn drop(&mut self) {
        // SAFETY: the uniquely owned recognizer is destroyed once.
        unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.recognizer.as_ptr()) };
    }
}
