//! Shared data carried between pipeline workers and across the IPC boundary.

use std::sync::Arc;

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
    Pause,
    SoftCut,
    HardCut,
    Discontinuity,
    End,
    /// The recognizer committed a clause at punctuation or the wait cap.
    Commit,
}

fn default_cut() -> CutReason {
    CutReason::Pause
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Segment {
    pub id: UtteranceId,
    pub start: StreamTime,
    pub end: StreamTime,
    /// Mono 16 kHz PCM, shared without copying between workers.
    pub samples: Arc<[f32]>,
    pub cut_reason: CutReason,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextClass {
    Chinese,
    Mixed,
    English,
    Other,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StageTiming {
    pub start: StreamTime,
    pub end: StreamTime,
    pub asr_done_ms: u64,
    pub asr_ms: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Transcript {
    pub id: UtteranceId,
    pub text: String,
    pub lang_tag: Option<String>,
    pub class: TextClass,
    pub event: Option<String>,
    pub timing: StageTiming,
    #[serde(default)]
    pub absorbed: Vec<UtteranceId>,
    /// How the clause ended. Early-commit clauses skip the short-phrase hold.
    #[serde(default = "default_cut")]
    pub cut: CutReason,
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

/// The caption mode that is actually running, as opposed to the one the user chose.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveMode {
    Continuous,
    Light,
    #[default]
    Off,
}

/// Why the effective mode differs from the chosen one, or how `auto` resolved.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeReason {
    User,
    Auto,
    Cpu,
    Lag,
    DraftUnavailable,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PipelineStats {
    pub lag_ms: u64,
    pub queue_depth: u32,
    pub held: u32,
    pub done_p50_ms: Option<u32>,
    pub done_p95_ms: Option<u32>,
    pub first_p50_ms: Option<u32>,
    pub skipped_total: u32,
    pub failed_total: u32,
    /// Percent of one CPU core used by the app over the last second.
    pub cpu_app_pct: f32,
    pub cpu_translator_pct: f32,
    pub rss_app_mb: u32,
    pub rss_translator_mb: u32,
    pub mode: EffectiveMode,
    pub mode_reason: Option<ModeReason>,
    /// Whole-PC CPU busy percent over the last second.
    pub cpu_system_pct: f32,
    /// Draft server, percent of one core.
    pub cpu_draft_pct: f32,
    pub rss_draft_mb: u32,
    pub word_first_p50_ms: Option<u32>,
    pub word_final_p50_ms: Option<u32>,
    pub drafts_total: u32,
    pub drafts_failed: u32,
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
