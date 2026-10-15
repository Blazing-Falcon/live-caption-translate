//! Shared data carried between pipeline workers and across the IPC boundary.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// All stream positions count mono samples at this rate.
pub const SAMPLE_RATE: u64 = 16_000;
pub const FRAME_SAMPLES: usize = 512;
/// JavaScript numbers represent integers exactly only through this value.
pub const MAX_UTTERANCE_ID: u64 = (1_u64 << 53) - 1;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UtteranceId(pub u64);

impl Serialize for UtteranceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.0 > MAX_UTTERANCE_ID {
            return Err(serde::ser::Error::custom(
                "utterance id exceeds the JavaScript safe integer range",
            ));
        }
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for UtteranceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        if value > MAX_UTTERANCE_ID {
            return Err(de::Error::custom(
                "utterance id exceeds the JavaScript safe integer range",
            ));
        }
        Ok(Self(value))
    }
}

/// A position on the monotonic 16 kHz stream clock, stored as samples.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct StreamTime(pub u64);

impl StreamTime {
    pub const ZERO: Self = Self(0);

    pub const fn from_samples(samples: u64) -> Self {
        Self(samples)
    }

    pub const fn samples(self) -> u64 {
        self.0
    }

    pub const fn from_millis(milliseconds: u64) -> Self {
        Self(milliseconds.saturating_mul(SAMPLE_RATE / 1_000))
    }

    /// Convert to whole milliseconds without an overflowing multiplication.
    pub const fn millis(self) -> u64 {
        self.0 / (SAMPLE_RATE / 1_000)
    }

    pub const fn as_millis(self) -> u64 {
        self.millis()
    }

    /// Round a nonnegative duration to the nearest sample. Negative/NaN becomes zero.
    pub fn from_seconds(seconds: f64) -> Self {
        Self((seconds * SAMPLE_RATE as f64).round() as u64)
    }

    pub fn seconds(self) -> f64 {
        self.0 as f64 / SAMPLE_RATE as f64
    }

    pub fn as_secs_f64(self) -> f64 {
        self.seconds()
    }

    pub const fn saturating_sub(self, earlier: Self) -> Self {
        Self(self.0.saturating_sub(earlier.0))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CutReason {
    /// The engine's pause rule.
    Pause,
    Discontinuity,
    End,
    /// The recognizer committed a clause at punctuation or the wait cap.
    Commit,
}

fn default_cut() -> CutReason {
    CutReason::Pause
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextClass {
    Chinese,
    Mixed,
    English,
    Other,
}

impl TextClass {
    /// Chinese and mixed lines go to the translator; the others show as heard.
    pub fn is_translated(self) -> bool {
        matches!(self, Self::Chinese | Self::Mixed)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StageTiming {
    pub start: StreamTime,
    pub end: StreamTime,
    pub asr_done_ms: u64,
    pub asr_ms: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecondPassOutcome {
    /// The final text comes from the second pass.
    Used,
    /// No part of the second pass's text matched the live text closely enough.
    Mismatch,
    /// The line waited too long for the second pass.
    Late,
    /// The second pass failed.
    Failed,
}

/// How the second pass read a closed clause. The line keeps the live text unless `outcome` is
/// `used`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SecondPass {
    pub outcome: SecondPassOutcome,
    /// The live text, when the final text differs from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_text: Option<String>,
    /// The second pass's text for the whole window, when it decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_text: Option<String>,
    /// Stream times of the decoded audio: the clause with a margin on both sides.
    pub window_start_ms: u64,
    pub window_end_ms: u64,
    /// Edit distance from the chosen part to the live text, over letters and digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode_ms: Option<u32>,
    /// From the clause's close to the final text, waiting included.
    pub refine_ms: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Transcript {
    pub id: UtteranceId,
    pub text: String,
    pub lang_tag: Option<String>,
    pub class: TextClass,
    pub timing: StageTiming,
    #[serde(default)]
    pub absorbed: Vec<UtteranceId>,
    /// How the clause ended.
    #[serde(default = "default_cut")]
    pub cut: CutReason,
    /// Mean token log-probability, when the engine gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprob: Option<f32>,
    /// Absent when the second pass is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_pass: Option<SecondPass>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    System,
    Apps,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceInfo {
    pub mode: CaptureMode,
    pub label: String,
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Timing {
    pub speech_end_ms: u64,
    pub asr_done_ms: u64,
    pub queued_ms: u64,
    pub sent_ms: u64,
    pub first_token_ms: Option<u64>,
    pub done_ms: u64,
    pub prompt_tokens: u32,
    pub cached_tokens: u32,
    pub generated_tokens: u32,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PipelineStats {
    pub lag_ms: u64,
    pub queue_depth: u32,
    pub done_p50_ms: Option<u32>,
    pub done_p95_ms: Option<u32>,
    pub first_p50_ms: Option<u32>,
    pub skipped_total: u32,
    pub failed_total: u32,
    /// Percent of one CPU core used by the app over the last second.
    pub cpu_app_pct: f32,
    pub cpu_translator_pct: f32,
    /// The second-pass server (Qwen3-ASR); zero when it is off.
    pub cpu_speech_pct: f32,
    pub rss_app_mb: u32,
    pub rss_translator_mb: u32,
    pub rss_speech_mb: u32,
    /// Whole-PC CPU busy percent over the last second.
    pub cpu_system_pct: f32,
    /// Median delay from a spoken word to the English that covers it, last minute.
    pub word_final_p50_ms: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utterance_ids_remain_exact_in_javascript() {
        let maximum = MAX_UTTERANCE_ID.to_string();
        assert_eq!(
            serde_json::from_str::<UtteranceId>(&maximum).unwrap(),
            UtteranceId(MAX_UTTERANCE_ID)
        );
        assert_eq!(
            serde_json::to_string(&UtteranceId(MAX_UTTERANCE_ID)).unwrap(),
            maximum
        );
        assert!(serde_json::from_str::<UtteranceId>("9007199254740992").is_err());
        assert!(serde_json::to_string(&UtteranceId(MAX_UTTERANCE_ID + 1)).is_err());
        assert!(serde_json::from_str::<UtteranceId>("-1").is_err());
        assert!(serde_json::from_str::<UtteranceId>("1.5").is_err());
    }
}
