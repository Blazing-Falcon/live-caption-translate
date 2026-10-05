use lt_core::{
    config::RoutingConfig,
    events::PipelineEvent,
    pipeline::SegmentCounts,
    types::{TextClass, UtteranceId},
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Complete-session latency samples, distinct from the UI's last-50 window.
pub struct ReplayReport {
    status: BTreeMap<&'static str, u64>,
    absorbed: BTreeMap<UtteranceId, u64>,
    ends: BTreeMap<UtteranceId, Vec<u64>>,
    translated_other: BTreeSet<String>,
    first: Vec<f64>,
    done: Vec<f64>,
    joins: BTreeMap<&'static str, u64>,
}

#[derive(Serialize)]
pub struct Summary<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub clip: &'a str,
    pub start: f64,
    pub dur: f64,
    pub segments: u64,
    pub cuts: BTreeMap<&'static str, u64>,
    pub status: BTreeMap<&'static str, u64>,
    pub lag_first_median: Option<f64>,
    pub lag_done_median: Option<f64>,
    pub lag_done_p90: Option<f64>,
    pub joins: BTreeMap<&'static str, u64>,
}

impl ReplayReport {
    pub fn new(routing: &RoutingConfig) -> Self {
        Self {
            status: [
                "final",
                "joined",
                "skipped",
                "passthrough",
                "untranslated",
                "dropped",
                "failed",
            ]
            .map(|key| (key, 0))
            .into(),
            absorbed: BTreeMap::new(),
            ends: BTreeMap::new(),
            translated_other: routing
                .translate_other
                .iter()
                .map(|tag| canonical_language(tag))
                .collect(),
            first: Vec::new(),
            done: Vec::new(),
            joins: [("hold", 0), ("queue", 0)].into(),
        }
    }
    fn terminal(&mut self, id: UtteranceId, state: &'static str) -> u64 {
        let count = self.absorbed.remove(&id).unwrap_or(0) + 1;
        *self.status.entry(state).or_default() += count;
        count
    }
    pub fn on_event(&mut self, event: &PipelineEvent) {
        match event {
            PipelineEvent::Joined {
                id, absorbed, kind, ..
            } => {
                let kind = match kind {
                    lt_core::events::JoinKind::Hold => "hold",
                    lt_core::events::JoinKind::Queue => "queue",
                };
                *self.joins.entry(kind).or_default() += absorbed.len() as u64;
                let mut count = self.absorbed.remove(id).unwrap_or(0);
                let mut ends = self.ends.remove(id).unwrap_or_default();
                for child in absorbed {
                    count += self.absorbed.remove(child).unwrap_or(0) + 1;
                    ends.extend(self.ends.remove(child).unwrap_or_default());
                }
                self.ends.insert(*id, ends);
                self.absorbed.insert(*id, count);
            }
            PipelineEvent::TranslationFinal { id, timing, .. } => {
                let state = if self.absorbed.contains_key(id) {
                    "joined"
                } else {
                    "final"
                };
                let constituents = self.terminal(*id, state);
                // The Python summary measures every constituent from its own
                // end, including members of queue joins. Preserve that policy.
                let ends = self
                    .ends
                    .remove(id)
                    .unwrap_or_else(|| vec![timing.speech_end_ms]);
                for end in ends {
                    self.done
                        .push(timing.done_ms.saturating_sub(end) as f64 / 1000.0);
                    if let Some(first) = timing.first_token_ms {
                        self.first.push(first.saturating_sub(end) as f64 / 1000.0);
                    }
                }
                tracing::debug!(id = id.0, constituents, "replay terminal");
            }
            PipelineEvent::Skipped { id, .. } => {
                self.terminal(*id, "skipped");
                self.ends.remove(id);
            }
            PipelineEvent::TranslationFailed { id, .. } => {
                self.terminal(*id, "failed");
                self.ends.remove(id);
            }
            PipelineEvent::Dropped { id, .. } => {
                self.terminal(*id, "dropped");
                self.ends.remove(id);
            }
            PipelineEvent::AsrFinal {
                id,
                class: TextClass::English,
                ..
            } => {
                self.terminal(*id, "passthrough");
            }
            PipelineEvent::AsrFinal {
                id,
                class: TextClass::Other,
                lang,
                end_ms,
                ..
            } => {
                let enabled = lang
                    .as_ref()
                    .is_some_and(|tag| self.translated_other.contains(&canonical_language(tag)));
                if enabled {
                    self.ends.insert(*id, vec![*end_ms]);
                } else {
                    self.terminal(*id, "untranslated");
                }
            }
            PipelineEvent::AsrFinal { id, end_ms, .. } => {
                self.ends.insert(*id, vec![*end_ms]);
            }
            _ => {}
        }
    }
    pub fn summary<'a>(
        self,
        counts: SegmentCounts,
        clip: &'a str,
        start: f64,
        dur: f64,
    ) -> Summary<'a> {
        Summary {
            kind: "summary",
            clip,
            start,
            dur,
            segments: counts.total(),
            cuts: [
                ("pause", counts.pause),
                ("soft cut", counts.soft_cut),
                ("hard cut", counts.hard_cut),
                ("discontinuity", counts.discontinuity),
                ("end", counts.end),
            ]
            .into(),
            status: self.status,
            lag_first_median: percentile(self.first, 0.5),
            lag_done_median: percentile(self.done.clone(), 0.5),
            lag_done_p90: percentile(self.done, 0.9),
            joins: self.joins,
        }
    }
}

// Keep the replay summary's Other routing aligned with pipeline/subscriber
// normalization for model tokens, language names and regional tags.
fn canonical_language(tag: &str) -> String {
    let tag = tag.trim();
    let tag = tag
        .strip_prefix("<|")
        .and_then(|tag| tag.strip_suffix("|>"))
        .unwrap_or(tag);
    let lowered = tag.to_ascii_lowercase();
    let language = lowered.split(['-', '_']).next().unwrap_or(&lowered);
    match language {
        "japanese" | "jpn" => "ja".into(),
        "korean" | "kor" => "ko".into(),
        language => language.into(),
    }
}

fn percentile(mut samples: Vec<f64>, fraction: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f64::total_cmp);
    let at = (samples.len() - 1) as f64 * fraction;
    let left = at.floor() as usize;
    let right = at.ceil() as usize;
    Some(samples[left] + (samples[right] - samples[left]) * at.fract())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::{
        events::{DropReason, JoinKind},
        types::Timing,
    };

    fn other(id: u64, lang: &str, end_ms: u64) -> PipelineEvent {
        PipelineEvent::AsrFinal {
            id: UtteranceId(id),
            text: "日本語".into(),
            class: TextClass::Other,
            lang: Some(lang.into()),
            start_ms: 0,
            end_ms,
            asr_ms: 1,
        }
    }

    #[test]
    fn regional_and_named_other_languages_count_once_and_keep_join_latencies() {
        let routing = RoutingConfig {
            translate_other: vec![" Japanese ".into(), "ko-KR".into()],
            ..RoutingConfig::default()
        };
        let mut report = ReplayReport::new(&routing);
        report.on_event(&other(1, "<|ja-JP|>", 1000));
        report.on_event(&other(2, "Korean", 2000));
        report.on_event(&PipelineEvent::Joined {
            id: UtteranceId(1),
            absorbed: vec![UtteranceId(2)],
            text: "combined".into(),
            kind: JoinKind::Queue,
        });
        report.on_event(&PipelineEvent::TranslationFinal {
            id: UtteranceId(1),
            text: "English".into(),
            timing: Timing {
                speech_end_ms: 2000,
                first_token_ms: Some(3000),
                done_ms: 4000,
                ..Timing::default()
            },
        });
        assert!(report.ends.is_empty());
        let summary = report.summary(SegmentCounts::default(), "fixture", 0.0, 4.0);
        assert_eq!(summary.status["joined"], 2);
        assert_eq!(summary.status["untranslated"], 0);
        assert_eq!(summary.lag_first_median, Some(1.5));
        assert_eq!(summary.lag_done_median, Some(2.5));
        assert_eq!(summary.lag_done_p90, Some(2.9));
    }

    #[test]
    fn dropped_join_removes_all_ends_and_disabled_other_is_terminal() {
        let mut report = ReplayReport::new(&RoutingConfig::default());
        for id in 1..=2 {
            report.on_event(&PipelineEvent::AsrFinal {
                id: UtteranceId(id),
                text: "你好".into(),
                class: TextClass::Chinese,
                lang: Some("zh".into()),
                start_ms: 0,
                end_ms: id * 1000,
                asr_ms: 1,
            });
        }
        report.on_event(&PipelineEvent::Joined {
            id: UtteranceId(1),
            absorbed: vec![UtteranceId(2)],
            text: "你好，你好".into(),
            kind: JoinKind::Hold,
        });
        report.on_event(&PipelineEvent::Dropped {
            id: UtteranceId(1),
            reason: DropReason::Empty,
        });
        report.on_event(&other(3, "ja", 3000));
        assert!(report.ends.is_empty());
        assert!(report.absorbed.is_empty());
        let summary = report.summary(SegmentCounts::default(), "fixture", 0.0, 4.0);
        assert_eq!(summary.status["dropped"], 2);
        assert_eq!(summary.status["untranslated"], 1);
        assert_eq!(summary.lag_done_median, None);
    }
}
