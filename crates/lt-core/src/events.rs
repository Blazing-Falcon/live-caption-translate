//! The snake_case, internally tagged wire contract shared with the frontend.

use serde::{Deserialize, Serialize};

pub use crate::types::{CaptureMode, PipelineStats, SourceInfo, TextClass, Timing, UtteranceId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinKind {
    Hold,
    Queue,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    CatchUp,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailReason {
    Timeout,
    ServerUnavailable,
    Echo,
    Runaway,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    Empty,
    Music,
    SingleChar,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Vad,
    Asr,
    Translator,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    Loading,
    Ready,
    Restarting,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStateKind {
    Playing,
    Silent,
    NoDevice,
    NoAppsSelected,
    AppsNotRunning,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListeningStateKind {
    Starting,
    Listening,
    Paused,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEvent {
    SpeechStarted {
        id: UtteranceId,
        at_ms: u64,
    },
    /// Reserved for a future streaming recognizer; v1 never emits it.
    AsrPartial {
        id: UtteranceId,
        text: String,
    },
    AsrFinal {
        id: UtteranceId,
        text: String,
        class: TextClass,
        lang: Option<String>,
        start_ms: u64,
        end_ms: u64,
        asr_ms: u32,
    },
    Joined {
        id: UtteranceId,
        absorbed: Vec<UtteranceId>,
        text: String,
        kind: JoinKind,
    },
    TranslationDelta {
        id: UtteranceId,
        text_so_far: String,
    },
    TranslationFinal {
        id: UtteranceId,
        text: String,
        timing: Timing,
    },
    Skipped {
        id: UtteranceId,
        reason: SkipReason,
    },
    TranslationFailed {
        id: UtteranceId,
        reason: FailReason,
        message: String,
    },
    Dropped {
        id: UtteranceId,
        reason: DropReason,
    },
    SourceChanged {
        info: SourceInfo,
    },
    SourceState {
        state: SourceStateKind,
        detail: Option<String>,
    },
    ListeningState {
        state: ListeningStateKind,
    },
    EngineStatus {
        engine: EngineKind,
        state: EngineState,
        message: Option<String>,
    },
    /// Internal tagging puts the stats fields beside `type`, without a payload wrapper.
    Stats(PipelineStats),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_event_variant() -> Vec<PipelineEvent> {
        vec![
            PipelineEvent::SpeechStarted {
                id: UtteranceId(12),
                at_ms: 45_210,
            },
            PipelineEvent::AsrPartial {
                id: UtteranceId(12),
                text: "我也想".into(),
            },
            PipelineEvent::AsrFinal {
                id: UtteranceId(12),
                text: "我也想办一个。".into(),
                class: TextClass::Chinese,
                lang: Some("zh".into()),
                start_ms: 45_210,
                end_ms: 46_300,
                asr_ms: 330,
            },
            PipelineEvent::Joined {
                id: UtteranceId(12),
                absorbed: vec![UtteranceId(13)],
                text: "我也想办一个，伟大的公司。".into(),
                kind: JoinKind::Hold,
            },
            PipelineEvent::TranslationDelta {
                id: UtteranceId(12),
                text_so_far: "I also want to run".into(),
            },
            PipelineEvent::TranslationFinal {
                id: UtteranceId(12),
                text: "I also want to run a great company.".into(),
                timing: Timing {
                    speech_end_ms: 47_980,
                    asr_done_ms: 48_310,
                    queued_ms: 48_320,
                    sent_ms: 48_320,
                    first_token_ms: Some(48_790),
                    done_ms: 49_210,
                    prompt_tokens: 17,
                    cached_tokens: 31,
                    generated_tokens: 9,
                },
            },
            PipelineEvent::Skipped {
                id: UtteranceId(14),
                reason: SkipReason::CatchUp,
            },
            PipelineEvent::TranslationFailed {
                id: UtteranceId(15),
                reason: FailReason::Timeout,
                message: "Translation took too long".into(),
            },
            PipelineEvent::Dropped {
                id: UtteranceId(16),
                reason: DropReason::Empty,
            },
            PipelineEvent::SourceChanged {
                info: SourceInfo {
                    mode: CaptureMode::Apps,
                    label: "Chrome, Discord".into(),
                    sample_rate: 48_000,
                    channels: 2,
                },
            },
            PipelineEvent::SourceState {
                state: SourceStateKind::Silent,
                detail: Some("Chrome, Discord".into()),
            },
            PipelineEvent::ListeningState {
                state: ListeningStateKind::Listening,
            },
            PipelineEvent::EngineStatus {
                engine: EngineKind::Translator,
                state: EngineState::Ready,
                message: None,
            },
            PipelineEvent::Stats(PipelineStats {
                lag_ms: 400,
                queue_depth: 1,
                held: 0,
                done_p50_ms: Some(1_230),
                done_p95_ms: Some(2_100),
                first_p50_ms: Some(700),
                skipped_total: 2,
                failed_total: 1,
                cpu_app_pct: 2.5,
                cpu_translator_pct: 150.0,
                rss_app_mb: 520,
                rss_translator_mb: 1_380,
            }),
        ]
    }

    #[test]
    fn every_event_matches_the_shared_wire_fixtures() {
        let rust_fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/events.json")).unwrap();
        let frontend_fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../app/ui/src/lib/__fixtures__/events.json"
        ))
        .unwrap();
        let events = every_event_variant();
        assert_eq!(events.len(), 14);
        assert_eq!(serde_json::to_value(&events).unwrap(), rust_fixture);
        assert_eq!(rust_fixture, frontend_fixture);
        assert_eq!(
            serde_json::from_value::<Vec<PipelineEvent>>(rust_fixture).unwrap(),
            events
        );
    }

    #[test]
    fn stats_are_flat_and_optional_values_are_explicit_nulls() {
        let value = serde_json::to_value(PipelineEvent::Stats(PipelineStats::default())).unwrap();
        assert_eq!(value["type"], "stats");
        assert_eq!(value["queue_depth"], 0);
        assert!(value.get("stats").is_none());
        assert!(value.get("payload").is_none());
        assert!(value["done_p50_ms"].is_null());
    }
}
