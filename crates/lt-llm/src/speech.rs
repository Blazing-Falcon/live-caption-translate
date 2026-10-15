//! Qwen3-ASR in its own llama-server: the second-pass recognizer. llama.cpp keeps it in about
//! two thirds of the memory that ONNX Runtime needs for the same model, and its memory does
//! not grow with the longest window it has read.

use crate::{
    client::{local_agent, Endpoint},
    supervisor::{ServerOptions, Supervisor},
};
use lt_core::{
    bus::EventBus,
    config::AsrConfig,
    engines::{OfflineAsr, OfflineText, TranslationControl},
    error::{Error, Result},
};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32},
        Arc,
    },
    time::{Duration, Instant},
};

/// The manifest model of `asr.second_pass = "qwen3"`.
pub const QWEN3_MODEL: &str = "qwen3-asr-0.6b-q4_k_m";
const MODEL_FILE: &str = "Qwen3-ASR-0.6B.Q4_K_M.gguf";
const MMPROJ_FILE: &str = "mmproj-Qwen3-ASR-0.6B-Q8_0.gguf";
/// A 30 s window takes a few seconds on two threads; a slower answer means the server hangs.
const DECODE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest text of one window; a 30 s clause of fast Chinese stays below it.
const MAX_TOKENS: u32 = 256;
/// Qwen3-ASR writes its answer as `language <Name><asr_text><text>`.
const TEXT_MARK: &str = "<asr_text>";
const LANGUAGES: [(&str, &str); 5] = [
    ("Chinese", "zh"),
    ("English", "en"),
    ("Cantonese", "yue"),
    ("Japanese", "ja"),
    ("Korean", "ko"),
];

pub struct Qwen3Server {
    endpoint: Endpoint,
    // Stops the server when the engine is dropped with its pipeline.
    supervisor: Supervisor,
}

impl Qwen3Server {
    /// Starts the server for the model in `dir` and waits until it answers.
    pub fn start(
        binary: &Path,
        dir: &Path,
        settings: &AsrConfig,
        below_normal: bool,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        let options = ServerOptions::speech(
            binary.to_path_buf(),
            dir.join(MODEL_FILE),
            dir.join(MMPROJ_FILE),
            settings.second_pass_engine_threads(),
        )
        .with_below_normal(below_normal);
        let supervisor = Supervisor::start(options, EventBus::default())?;
        supervisor.wait_ready_cancellable(Duration::from_secs(65), cancelled)?;
        Ok(Self {
            endpoint: Endpoint::new(supervisor.url())?,
            supervisor,
        })
    }

    /// The server's pid for resource stats, zero after it stopped.
    pub fn child_pid(&self) -> Arc<AtomicU32> {
        self.supervisor.child_pid()
    }
}

impl OfflineAsr for Qwen3Server {
    /// A forced language starts the answer with its name, so the model writes only the text.
    fn decode(&mut self, samples: &[f32], language: Option<&str>) -> Result<OfflineText> {
        let audio = json!({
            "type": "input_audio",
            "input_audio": {"data": base64(&wav(samples)), "format": "wav"},
        });
        let mut messages = vec![json!({"role": "user", "content": [audio]})];
        let forced = language.and_then(|code| {
            LANGUAGES
                .iter()
                .find(|(_, known)| *known == code)
                .map(|(name, _)| *name)
        });
        if let Some(name) = forced {
            messages.push(
                json!({"role": "assistant", "content": format!("language {name}{TEXT_MARK}")}),
            );
        }
        let body = json!({
            "messages": messages,
            "temperature": 0.0,
            "max_tokens": MAX_TOKENS,
            "cache_prompt": false,
        });
        let control = TranslationControl {
            deadline: Instant::now() + DECODE_TIMEOUT,
            cancelled: Arc::new(AtomicBool::new(false)),
            abort: Arc::new(AtomicBool::new(false)),
        };
        let failed = |error: &dyn std::fmt::Display| {
            Error::Engine(format!("Second-pass recognizer request failed: {error}"))
        };
        let mut response = local_agent(self.endpoint.address, control)?
            .post(&self.endpoint.completions)
            .send_json(body)
            .map_err(|error| failed(&error))?;
        if response.status().as_u16() != 200 {
            return Err(failed(&format!("HTTP {}", response.status())));
        }
        let reply: Value = response
            .body_mut()
            .read_json()
            .map_err(|error| failed(&error))?;
        let content = reply["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| failed(&"no text in the answer"))?;
        Ok(parse_answer(content, forced))
    }
}

/// The text and language of an answer. With a forced language the server may return only the
/// text that follows it.
fn parse_answer(content: &str, forced: Option<&str>) -> OfflineText {
    let (name, text) = match content.split_once(TEXT_MARK) {
        Some((head, text)) => (head.trim().strip_prefix("language").map(str::trim), text),
        None => (forced, content),
    };
    let lang = name.and_then(|name| {
        LANGUAGES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, code)| (*code).to_owned())
    });
    OfflineText {
        text: text.trim().to_owned(),
        lang,
    }
}

/// 16-bit PCM WAV, 16 kHz mono.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&16_000_u32.to_le_bytes());
    out.extend_from_slice(&32_000_u32.to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    ALPHABET[(value >> (18 - 6 * index)) as usize & 63],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answer format is the contract with the model; a wrong split would keep the live text
    /// of every line without a visible error.
    #[test]
    fn answers_give_text_and_language_with_and_without_a_forced_language() {
        let heard = parse_answer("language Chinese<asr_text>饭不能在电话里吃。", None);
        assert_eq!(heard.text, "饭不能在电话里吃。");
        assert_eq!(heard.lang.as_deref(), Some("zh"));
        let forced = parse_answer("饭不能在电话里吃。", Some("Chinese"));
        assert_eq!(
            (forced.text.as_str(), forced.lang.as_deref()),
            ("饭不能在电话里吃。", Some("zh"))
        );
        let other = parse_answer("language None<asr_text>", None);
        assert_eq!((other.text.as_str(), other.lang), ("", None));
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
