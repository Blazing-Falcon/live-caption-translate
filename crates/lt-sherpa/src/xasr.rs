//! X-ASR: an offline zipformer transducer (Chinese, English, punctuation), greedy search.
//! It fills per-token log-probabilities and gives no language tag.

use crate::offline::{cstring, path_cstring, require_threads, zeroed_config, OfflineEngine};
use lt_core::{
    engines::{mean_log_prob, SegmentAsr, TokenTranscript, WindowAsr},
    error::Result,
    text::{classify, clean},
    types::{Segment, StageTiming, TextClass, Transcript},
};
use std::{path::Path, time::Instant};

const NAME: &str = "X-ASR";

pub struct XAsr {
    engine: OfflineEngine,
}

impl XAsr {
    /// `dir` holds the encoder, decoder, joiner and tokens of the manifest entry.
    pub fn new(dir: impl AsRef<Path>, threads: u32) -> Result<Self> {
        Self::open(dir.as_ref(), threads, None)
    }

    pub fn with_runtime(
        dir: impl AsRef<Path>,
        threads: u32,
        runtime: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open(dir.as_ref(), threads, Some(runtime.as_ref()))
    }

    fn open(dir: &Path, threads: u32, runtime: Option<&Path>) -> Result<Self> {
        require_threads(NAME, threads)?;
        let encoder = dir.join("encoder-epoch-99-avg-1.int8.onnx");
        let decoder = dir.join("decoder-epoch-99-avg-1.onnx");
        let joiner = dir.join("joiner-epoch-99-avg-1.int8.onnx");
        let tokens = dir.join("tokens.txt");
        OfflineEngine::prepare(NAME, runtime, &[&encoder, &decoder, &joiner, &tokens])?;
        let encoder = path_cstring(&encoder, "X-ASR encoder path")?;
        let decoder = path_cstring(&decoder, "X-ASR decoder path")?;
        let joiner = path_cstring(&joiner, "X-ASR joiner path")?;
        let tokens = path_cstring(&tokens, "X-ASR tokens path")?;
        let provider = cstring("cpu")?;
        let decoding = cstring("greedy_search")?;
        let model_type = cstring("transducer")?;
        let mut config = zeroed_config();
        config.model_config.model_type = model_type.as_ptr();
        config.model_config.transducer.encoder = encoder.as_ptr();
        config.model_config.transducer.decoder = decoder.as_ptr();
        config.model_config.transducer.joiner = joiner.as_ptr();
        config.model_config.tokens = tokens.as_ptr();
        config.model_config.num_threads = threads as i32;
        config.model_config.provider = provider.as_ptr();
        config.decoding_method = decoding.as_ptr();
        config.max_active_paths = 4;
        Ok(Self {
            engine: OfflineEngine::create(NAME, &config)?,
        })
    }
}

impl WindowAsr for XAsr {
    fn decode_window(&mut self, samples: &[f32]) -> Result<TokenTranscript> {
        let mut transcript = self.engine.decode(samples)?.into_tokens();
        normalize_tokens(&mut transcript.tokens);
        Ok(transcript)
    }
}

impl SegmentAsr for XAsr {
    fn window(&mut self) -> Option<&mut dyn WindowAsr> {
        Some(self)
    }

    fn transcribe(&mut self, segment: &Segment) -> Result<Transcript> {
        let started = Instant::now();
        let decoded = self.decode_window(&segment.samples)?;
        let class = classify(&clean(&decoded.text, &[])).unwrap_or(TextClass::Chinese);
        Ok(Transcript {
            id: segment.id,
            logprob: mean_log_prob(&decoded.log_probs),
            text: decoded.text,
            lang_tag: None,
            class,
            event: None,
            timing: StageTiming {
                start: segment.start,
                end: segment.end,
                asr_done_ms: 0,
                asr_ms: started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32,
            },
            absorbed: Vec::new(),
            cut: segment.cut_reason,
        })
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
