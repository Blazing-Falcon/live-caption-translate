//! SenseVoice: sherpa's offline recognizer, one decode for each window of audio. It reads the
//! window whole, so it hears each word with the words after it, which the streaming model
//! cannot.

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
    path::{Path, PathBuf},
    ptr::NonNull,
};

const NAME: &str = "SenseVoice";

pub struct SenseVoice {
    model: PathBuf,
    tokens: PathBuf,
    threads: u32,
    detect: Recognizer,
    /// sherpa reads SenseVoice's language when the recognizer is created (a stream option does
    /// not change it), so a forced language needs its own recognizer. It is created on first
    /// use: few windows need one, and each recognizer holds its own copy of the model.
    forced: Option<(String, Recognizer)>,
}

impl SenseVoice {
    /// `dir` holds the model and tokens of the manifest entry.
    pub fn new(dir: impl AsRef<Path>, settings: &AsrConfig) -> Result<Self> {
        let dir = dir.as_ref();
        let model = dir.join("model.int8.onnx");
        let tokens = dir.join("tokens.txt");
        prepare(NAME, None, &[&model, &tokens])?;
        let threads = settings.second_pass_engine_threads();
        let detect = Recognizer::new(&model, &tokens, "auto", threads)?;
        Ok(Self {
            model,
            tokens,
            threads,
            detect,
            forced: None,
        })
    }
}

impl OfflineAsr for SenseVoice {
    fn decode(&mut self, samples: &[f32], language: Option<&str>) -> Result<OfflineText> {
        let Some(language) = language else {
            return self.detect.decode(samples);
        };
        let forced = match self.forced.take() {
            Some(forced) if forced.0 == language => forced,
            _ => (
                language.to_owned(),
                Recognizer::new(&self.model, &self.tokens, language, self.threads)?,
            ),
        };
        let heard = forced.1.decode(samples);
        self.forced = Some(forced);
        heard
    }
}

struct Recognizer(NonNull<sys::OfflineRecognizer>);

// SAFETY: the recognizer moves to one worker with its engine, and every call takes the engine
// by `&mut self`. It is never shared between threads; no Sync implementation is provided.
unsafe impl Send for Recognizer {}

impl Recognizer {
    fn new(model: &Path, tokens: &Path, language: &str, threads: u32) -> Result<Self> {
        let model = path_cstring(model, "SenseVoice model path")?;
        let tokens = path_cstring(tokens, "SenseVoice tokens path")?;
        let language = cstring(language)?;
        let provider = cstring("cpu")?;
        let decoding = cstring("greedy_search")?;
        // SAFETY: this repr(C) config and its nested structs contain only raw pointers, integers
        // and floats; the all-zero representation is valid and selects no other model family.
        let mut config: sys::OfflineRecognizerConfig = unsafe { std::mem::zeroed() };
        config.feat_config.sample_rate = 16_000;
        config.feat_config.feature_dim = 80;
        config.model_config.sense_voice.model = model.as_ptr();
        config.model_config.sense_voice.language = language.as_ptr();
        // Inverse text normalization gives punctuation and digits, like the live text.
        config.model_config.sense_voice.use_itn = 1;
        config.model_config.tokens = tokens.as_ptr();
        config.model_config.num_threads = threads as i32;
        config.model_config.provider = provider.as_ptr();
        config.decoding_method = decoding.as_ptr();
        // SAFETY: every pointer above is a live CString through this call; the native
        // constructor copies the strings.
        let pointer = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(&config) };
        NonNull::new(pointer.cast_mut()).map(Self).ok_or_else(|| {
            Error::Engine(format!(
                "{NAME} could not initialize; check its model files"
            ))
        })
    }

    fn decode(&self, samples: &[f32]) -> Result<OfflineText> {
        let count = i32::try_from(samples.len())
            .map_err(|_| Error::Engine(format!("{NAME} audio window is too long")))?;
        // SAFETY: the recognizer is live; the stream is destroyed below, before it.
        let stream = unsafe { sys::SherpaOnnxCreateOfflineStream(self.0.as_ptr()) };
        let stream = NonNull::new(stream.cast_mut())
            .ok_or_else(|| Error::Engine(format!("{NAME} could not create an audio stream")))?;
        // SAFETY: both handles are live and used only on this worker. The stream copies the
        // samples during the call.
        let json = unsafe {
            sys::SherpaOnnxAcceptWaveformOffline(stream.as_ptr(), 16_000, samples.as_ptr(), count);
            sys::SherpaOnnxDecodeOfflineStream(self.0.as_ptr(), stream.as_ptr());
            sys::SherpaOnnxGetOfflineStreamResultAsJson(stream.as_ptr())
        };
        let parsed = match NonNull::new(json.cast_mut()) {
            Some(json) => {
                // SAFETY: a NUL-terminated string, valid until destroyed below.
                let text = unsafe { CStr::from_ptr(json.as_ptr().cast::<c_char>()) };
                let parsed = parse_result(&text.to_string_lossy());
                // SAFETY: the matching destroy function, called once.
                unsafe { sys::SherpaOnnxDestroyOfflineStreamResultJson(json.as_ptr()) };
                parsed
            }
            None => Err(Error::Engine(format!("{NAME} returned no result"))),
        };
        // SAFETY: destroyed once, after its result.
        unsafe { sys::SherpaOnnxDestroyOfflineStream(stream.as_ptr()) };
        parsed
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        // SAFETY: this uniquely owned handle has not been destroyed; its streams are gone.
        unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.0.as_ptr()) }
    }
}

#[derive(Deserialize)]
struct RawResult {
    #[serde(default)]
    text: String,
    #[serde(default)]
    lang: String,
}

/// The language tag comes as `<|zh|>`.
fn parse_result(json: &str) -> Result<OfflineText> {
    let raw: RawResult = serde_json::from_str(json)
        .map_err(|error| engine_error(&format!("{NAME} returned invalid result JSON"), error))?;
    let lang = raw
        .lang
        .trim()
        .trim_start_matches("<|")
        .trim_end_matches("|>")
        .to_owned();
    Ok(OfflineText {
        text: raw.text,
        lang: (!lang.is_empty()).then_some(lang),
    })
}
