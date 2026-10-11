//! Streaming X-ASR: sherpa's online zipformer transducer. It hears all audio continuously,
//! keeps its state between pieces, and its emitted tokens never change.

use crate::runtime::{cstring, engine_error, path_cstring, prepare};
use lt_core::{
    config::AsrConfig,
    engines::{StreamResult, StreamingAsr, TokenTranscript},
    error::{Error, Result},
};
use serde::Deserialize;
use sherpa_onnx_sys as sys;
use std::{
    ffi::{c_char, CStr},
    path::Path,
    ptr::NonNull,
};

const NAME: &str = "X-ASR streaming";
/// Silence that ends an utterance when nothing was said (sherpa's default).
const IDLE_ENDPOINT_S: f32 = 2.4;
/// Longest utterance before sherpa resets. Its cut can split a word, so it is far above the
/// commit rule's cap, which normally ends clauses first.
const MAX_UTTERANCE_S: f32 = 60.0;

pub struct XAsrStream {
    recognizer: NonNull<sys::OnlineRecognizer>,
    stream: Option<NonNull<sys::OnlineStream>>,
    /// Silence added before the end of input: the encoder needs a full chunk plus look-ahead.
    tail: Vec<f32>,
    /// Seconds the model reports a word after it starts; subtracted from token times.
    lag_s: f32,
}

/// The median delay of this model's first word after silence behind Silero's speech onset, for
/// each chunk size. Times count audio frames, so the delay does not depend on the PC.
fn token_lag_s(chunk_ms: u32) -> f32 {
    match chunk_ms {
        160 => 0.44,
        960 => 0.23,
        _ => 0.34,
    }
}

// SAFETY: the recognizer and its stream move to one ASR worker together and every call takes
// `&mut self`. They are never shared between threads; no Sync implementation is provided.
unsafe impl Send for XAsrStream {}

impl XAsrStream {
    /// `dir` holds the encoder, decoder, joiner and tokens of the manifest entry.
    pub fn new(dir: impl AsRef<Path>, settings: &AsrConfig) -> Result<Self> {
        let dir = dir.as_ref();
        let encoder = dir.join("encoder.int8.onnx");
        let decoder = dir.join("decoder.onnx");
        let joiner = dir.join("joiner.int8.onnx");
        let tokens = dir.join("tokens.txt");
        prepare(NAME, None, &[&encoder, &decoder, &joiner, &tokens])?;
        let encoder = path_cstring(&encoder, "X-ASR streaming encoder path")?;
        let decoder = path_cstring(&decoder, "X-ASR streaming decoder path")?;
        let joiner = path_cstring(&joiner, "X-ASR streaming joiner path")?;
        let tokens = path_cstring(&tokens, "X-ASR streaming tokens path")?;
        let provider = cstring("cpu")?;
        let decoding = cstring("greedy_search")?;
        // SAFETY: this repr(C) config contains only raw pointers, integers and floats; the
        // all-zero representation is valid and selects no other model family.
        let mut config: sys::OnlineRecognizerConfig = unsafe { std::mem::zeroed() };
        config.feat_config.sample_rate = 16_000;
        config.feat_config.feature_dim = 80;
        config.model_config.transducer.encoder = encoder.as_ptr();
        config.model_config.transducer.decoder = decoder.as_ptr();
        config.model_config.transducer.joiner = joiner.as_ptr();
        config.model_config.tokens = tokens.as_ptr();
        config.model_config.num_threads = settings.engine_threads() as i32;
        config.model_config.provider = provider.as_ptr();
        config.decoding_method = decoding.as_ptr();
        config.max_active_paths = 4;
        config.enable_endpoint = 1;
        config.rule1_min_trailing_silence = IDLE_ENDPOINT_S;
        config.rule2_min_trailing_silence = settings.pause_s;
        config.rule3_min_utterance_length = MAX_UTTERANCE_S;
        // SAFETY: every pointer above is a live CString through this call; the native
        // constructor copies the strings.
        let pointer = unsafe { sys::SherpaOnnxCreateOnlineRecognizer(&config) };
        let recognizer = NonNull::new(pointer.cast_mut()).ok_or_else(|| {
            Error::Engine(format!(
                "{NAME} could not initialize; check its model files"
            ))
        })?;
        let chunk_s = f64::from(settings.chunk_ms) / 1_000.0;
        Ok(Self {
            recognizer,
            stream: None,
            tail: vec![0.0; ((2.0 * chunk_s + 0.5) * 16_000.0) as usize],
            lag_s: token_lag_s(settings.chunk_ms),
        })
    }

    fn stream(&mut self) -> Result<NonNull<sys::OnlineStream>> {
        if let Some(stream) = self.stream {
            return Ok(stream);
        }
        // SAFETY: the recognizer is live; the stream is destroyed before it in Drop.
        let pointer = unsafe { sys::SherpaOnnxCreateOnlineStream(self.recognizer.as_ptr()) };
        let stream = NonNull::new(pointer.cast_mut())
            .ok_or_else(|| Error::Engine(format!("{NAME} could not create an audio stream")))?;
        self.stream = Some(stream);
        Ok(stream)
    }

    fn feed(&mut self, samples: &[f32]) -> Result<()> {
        if samples.len() > i32::MAX as usize {
            return Err(Error::Engine(format!("{NAME} audio piece is too long")));
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Engine(format!(
                "{NAME} received nonfinite audio samples"
            )));
        }
        let stream = self.stream()?;
        // SAFETY: both handles are live and used only on this worker; the stream copies the
        // samples during the call.
        unsafe {
            sys::SherpaOnnxOnlineStreamAcceptWaveform(
                stream.as_ptr(),
                16_000,
                samples.as_ptr(),
                samples.len() as i32,
            );
        }
        self.decode_ready(stream);
        Ok(())
    }

    fn decode_ready(&self, stream: NonNull<sys::OnlineStream>) {
        // SAFETY: both handles are live and used only on this worker.
        unsafe {
            while sys::SherpaOnnxIsOnlineStreamReady(self.recognizer.as_ptr(), stream.as_ptr()) != 0
            {
                sys::SherpaOnnxDecodeOnlineStream(self.recognizer.as_ptr(), stream.as_ptr());
            }
        }
    }

    fn read(&self, stream: NonNull<sys::OnlineStream>) -> Result<StreamResult> {
        // SAFETY: the live handles return a separately allocated JSON string, freed below.
        let pointer = unsafe {
            sys::SherpaOnnxGetOnlineStreamResultAsJson(self.recognizer.as_ptr(), stream.as_ptr())
        };
        let pointer = NonNull::new(pointer.cast_mut())
            .ok_or_else(|| Error::Engine(format!("{NAME} returned no result")))?;
        let parsed = {
            // SAFETY: NUL-terminated string, valid until destroyed below.
            let json = unsafe { CStr::from_ptr(pointer.as_ptr().cast::<c_char>()) };
            json.to_str()
                .map_err(|error| engine_error(&format!("{NAME} result is not UTF-8"), error))
                .and_then(|json| parse_result(json, self.lag_s))
        };
        // SAFETY: the matching destroy function, called once.
        unsafe { sys::SherpaOnnxDestroyOnlineStreamResultJson(pointer.as_ptr()) };
        parsed
    }
}

#[derive(Deserialize)]
struct OnlineResult {
    #[serde(default)]
    text: String,
    #[serde(default)]
    tokens: Vec<String>,
    #[serde(default)]
    timestamps: Vec<f32>,
    #[serde(default)]
    ys_probs: Vec<f32>,
    #[serde(default)]
    start_time: f64,
}

fn parse_result(json: &str, lag_s: f32) -> Result<StreamResult> {
    let raw: OnlineResult = serde_json::from_str(json)
        .map_err(|error| engine_error(&format!("{NAME} returned invalid result JSON"), error))?;
    let mut tokens = raw.tokens;
    normalize_tokens(&mut tokens);
    Ok(StreamResult {
        start_s: raw.start_time,
        tokens: TokenTranscript {
            text: raw.text,
            tokens,
            timestamps: raw
                .timestamps
                .iter()
                .map(|time| (time - lag_s).max(0.0))
                .collect(),
            log_probs: raw.ys_probs,
        },
    })
}

impl StreamingAsr for XAsrStream {
    fn accept(&mut self, samples: &[f32]) -> Result<()> {
        self.feed(samples)
    }

    fn result(&mut self) -> Result<StreamResult> {
        match self.stream {
            Some(stream) => self.read(stream),
            None => Ok(StreamResult::default()),
        }
    }

    fn is_endpoint(&mut self) -> bool {
        self.stream.is_some_and(|stream| {
            // SAFETY: both handles are live and used only on this worker.
            unsafe {
                sys::SherpaOnnxOnlineStreamIsEndpoint(self.recognizer.as_ptr(), stream.as_ptr())
                    != 0
            }
        })
    }

    fn reset(&mut self) {
        if let Some(stream) = self.stream {
            // SAFETY: both handles are live and used only on this worker.
            unsafe { sys::SherpaOnnxOnlineStreamReset(self.recognizer.as_ptr(), stream.as_ptr()) }
        }
    }

    fn finish(&mut self) -> Result<StreamResult> {
        let Some(stream) = self.stream else {
            return Ok(StreamResult::default());
        };
        let tail = std::mem::take(&mut self.tail);
        let fed = self.feed(&tail);
        self.tail = tail;
        fed?;
        // SAFETY: the live stream; no audio follows this call.
        unsafe { sys::SherpaOnnxOnlineStreamInputFinished(stream.as_ptr()) };
        self.decode_ready(stream);
        let result = self.read(stream);
        // SAFETY: destroyed once; the next accept creates a new stream.
        unsafe { sys::SherpaOnnxDestroyOnlineStream(stream.as_ptr()) };
        self.stream = None;
        result
    }
}

impl Drop for XAsrStream {
    fn drop(&mut self) {
        // SAFETY: each uniquely owned handle is destroyed once, the stream first.
        unsafe {
            if let Some(stream) = self.stream.take() {
                sys::SherpaOnnxDestroyOnlineStream(stream.as_ptr());
            }
            sys::SherpaOnnxDestroyOnlineRecognizer(self.recognizer.as_ptr());
        }
    }
}

/// sherpa marks every BPE piece that starts a word with a space, Chinese characters and
/// punctuation included. Keep the space only where the text has one (before an English word,
/// and between English and Chinese), so the tokens concatenate to the result text and a
/// punctuation mark is a one-character token for the commit rule.
pub(crate) fn normalize_tokens(tokens: &mut [String]) {
    let mut previous_ascii_word = false;
    for token in tokens.iter_mut() {
        let Some(first) = token.trim_start().chars().next() else {
            previous_ascii_word = false;
            continue;
        };
        let keep_space = first.is_ascii_alphanumeric()
            || (!first.is_ascii() && previous_ascii_word && !is_punctuation(first));
        if token.starts_with(' ') && !keep_space {
            *token = token.trim_start().to_owned();
        }
        let last = token.chars().next_back().unwrap_or(' ');
        previous_ascii_word = last.is_ascii_alphanumeric();
    }
}

fn is_punctuation(character: char) -> bool {
    !character.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_concatenate_to_the_native_text_and_marks_stand_alone() {
        // Recorded from the model's test_wavs/0.wav with sherpa-onnx 1.13.8.
        let mut tokens: Vec<String> = [
            " 昨", " 天", " 是", " ", "M", "on", "da", "y", " ,", " today", " is", " 礼", " 拜",
            " 二", " ，", " the", " day", " after", " to", "m", "or", "ro", "w", " 是", " 星",
            " 期", " 三", " 。",
        ]
        .map(String::from)
        .to_vec();
        normalize_tokens(&mut tokens);
        assert_eq!(
            tokens.concat().trim(),
            "昨天是 Monday, today is 礼拜二， the day after tomorrow 是星期三。"
        );
        assert_eq!(tokens.last().map(String::as_str), Some("。"));
        assert!(tokens.contains(&"，".to_owned()));
    }
}
