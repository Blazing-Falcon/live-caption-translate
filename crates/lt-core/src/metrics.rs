//! Event-derived metrics and injected process resource sampling.
use crate::events::PipelineEvent;
use crate::text::chinese_chars;
use crate::types::{PipelineStats, StreamTime, TextClass, UtteranceId};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const WINDOW_SIZE: usize = 50;

/// Shared routing for event subscribers: Other is terminal only if untranslated.
#[derive(Default)]
pub(crate) struct OtherRouting {
    enabled: BTreeSet<String>,
}

impl OtherRouting {
    pub(crate) fn set(&mut self, languages: &[String]) {
        self.enabled = languages
            .iter()
            .map(|language| canonical_language(language))
            .filter(|language| !language.is_empty())
            .collect();
    }

    pub(crate) fn translates(&self, language: Option<&str>) -> bool {
        language.is_some_and(|language| self.enabled.contains(&canonical_language(language)))
    }
}

pub(crate) fn canonical_language(tag: &str) -> String {
    let tag = tag.trim();
    let tag = tag
        .strip_prefix("<|")
        .and_then(|tag| tag.strip_suffix("|>"))
        .map_or(tag, |tag| tag);
    let lowered = tag.to_ascii_lowercase();
    let language = lowered
        .split(['-', '_'])
        .next()
        .map_or(lowered.as_str(), |language| language);
    match language {
        "japanese" | "jpn" => "ja".into(),
        "korean" | "kor" => "ko".into(),
        language => language.into(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ResourceSample {
    pub cpu_app_pct: f32,
    pub cpu_translator_pct: f32,
    pub rss_app_mb: u32,
    pub rss_translator_mb: u32,
}

/// The shell/CLI supplies OS-specific process counters; core stays portable.
pub trait ResourceSampler: Send {
    fn sample(&mut self) -> ResourceSample;
}

struct FinalSample {
    done_ms: u32,
    first_ms: Option<u32>,
}

#[derive(Default)]
pub struct Metrics {
    finals: VecDeque<FinalSample>,
    unfinished: BTreeMap<UtteranceId, u64>,
    skipped_total: u32,
    failed_total: u32,
    leaked_cjk_total: u64,
    sampler: Option<Box<dyn ResourceSampler>>,
    routing: OtherRouting,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_sampler(sampler: Box<dyn ResourceSampler>) -> Self {
        Self {
            sampler: Some(sampler),
            ..Self::default()
        }
    }

    pub fn with_routing(translate_other: &[String]) -> Self {
        let mut metrics = Self::new();
        metrics.set_translate_other(translate_other);
        metrics
    }

    /// Applies to subsequent ASR events; the pipeline restarts on routing changes.
    pub fn set_translate_other(&mut self, translate_other: &[String]) {
        self.routing.set(translate_other);
    }

    /// Internal audio/ASR seam; the wire event does not expose segment closure.
    pub fn segment_closed(&mut self, id: UtteranceId, end: StreamTime) {
        self.unfinished.insert(id, end.millis());
    }

    pub fn set_sampler(&mut self, sampler: Option<Box<dyn ResourceSampler>>) {
        self.sampler = sampler;
    }

    pub fn leaked_cjk_total(&self) -> u64 {
        self.leaked_cjk_total
    }

    pub fn record(&mut self, event: &PipelineEvent) {
        match event {
            PipelineEvent::AsrFinal {
                id,
                class,
                lang,
                end_ms,
                ..
            } => {
                if matches!(class, TextClass::Chinese | TextClass::Mixed)
                    || (*class == TextClass::Other && self.routing.translates(lang.as_deref()))
                {
                    self.unfinished.insert(*id, *end_ms);
                } else {
                    self.unfinished.remove(id);
                }
            }
            PipelineEvent::Joined { id, absorbed, .. } => {
                let mut end = self.unfinished.remove(id);
                for absorbed in absorbed {
                    if let Some(absorbed_end) = self.unfinished.remove(absorbed) {
                        end = Some(end.map_or(absorbed_end, |end| end.max(absorbed_end)));
                    }
                }
                if let Some(end) = end {
                    self.unfinished.insert(*id, end);
                }
            }
            PipelineEvent::TranslationFinal { id, text, timing } => {
                self.unfinished.remove(id);
                if self.finals.len() == WINDOW_SIZE {
                    self.finals.pop_front();
                }
                self.finals.push_back(FinalSample {
                    done_ms: milliseconds_u32(timing.done_ms.saturating_sub(timing.speech_end_ms)),
                    first_ms: timing
                        .first_token_ms
                        .map(|first| milliseconds_u32(first.saturating_sub(timing.speech_end_ms))),
                });
                self.leaked_cjk_total = self
                    .leaked_cjk_total
                    .saturating_add(chinese_chars(text) as u64);
            }
            PipelineEvent::Skipped { id, .. } => {
                self.unfinished.remove(id);
                self.skipped_total = self.skipped_total.saturating_add(1);
            }
            PipelineEvent::TranslationFailed { id, .. } => {
                self.unfinished.remove(id);
                self.failed_total = self.failed_total.saturating_add(1);
            }
            PipelineEvent::Dropped { id, .. } => {
                self.unfinished.remove(id);
            }
            _ => {}
        }
    }

    /// The caller requests snapshots on its once-per-second timer.
    pub fn snapshot(&mut self, live: StreamTime, queue_depth: u32, held: u32) -> PipelineStats {
        let done: Vec<_> = self.finals.iter().map(|sample| sample.done_ms).collect();
        let first: Vec<_> = self
            .finals
            .iter()
            .filter_map(|sample| sample.first_ms)
            .collect();
        let resources = self
            .sampler
            .as_mut()
            .map_or_else(ResourceSample::default, |sampler| sampler.sample());
        PipelineStats {
            lag_ms: self
                .unfinished
                .values()
                .min()
                .map_or(0, |end| live.millis().saturating_sub(*end)),
            queue_depth,
            held,
            done_p50_ms: nearest_rank(&done, 50),
            done_p95_ms: nearest_rank(&done, 95),
            first_p50_ms: nearest_rank(&first, 50),
            skipped_total: self.skipped_total,
            failed_total: self.failed_total,
            cpu_app_pct: resources.cpu_app_pct,
            cpu_translator_pct: resources.cpu_translator_pct,
            rss_app_mb: resources.rss_app_mb,
            rss_translator_mb: resources.rss_translator_mb,
        }
    }
}

fn milliseconds_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn nearest_rank(values: &[u32], percentile: usize) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = (percentile * sorted.len()).div_ceil(100);
    sorted.get(rank.saturating_sub(1)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{DropReason, FailReason, JoinKind, SkipReason};
    use crate::types::Timing;

    fn asr(id: u64, end: u64, class: TextClass) -> PipelineEvent {
        PipelineEvent::AsrFinal {
            id: UtteranceId(id),
            text: "我们。".into(),
            class,
            lang: Some("zh".into()),
            start_ms: end.saturating_sub(500),
            end_ms: end,
            asr_ms: 100,
        }
    }
    fn final_event(id: u64, done: u64, first: Option<u64>) -> PipelineEvent {
        PipelineEvent::TranslationFinal {
            id: UtteranceId(id),
            text: "We are here.".into(),
            timing: Timing {
                speech_end_ms: 1_000,
                done_ms: 1_000 + done,
                first_token_ms: first.map(|first| first + 1_000),
                ..Timing::default()
            },
        }
    }

    #[test]
    fn percentile_window_uses_nearest_rank_over_last_fifty_finals() {
        let mut metrics = Metrics::new();
        assert_eq!(
            metrics.snapshot(StreamTime::ZERO, 0, 0),
            PipelineStats::default()
        );
        for index in 1..=50 {
            metrics.record(&final_event(index, index * 100, Some(index * 10)));
        }
        let stats = metrics.snapshot(StreamTime::ZERO, 3, 2);
        assert_eq!(stats.done_p50_ms, Some(2_500));
        assert_eq!(stats.done_p95_ms, Some(4_800));
        assert_eq!(stats.first_p50_ms, Some(250));
        assert_eq!(stats.queue_depth, 3);
        assert_eq!(stats.held, 2);
        metrics.record(&final_event(51, 9_000, None));
        let stats = metrics.snapshot(StreamTime::ZERO, 0, 0);
        assert_eq!(stats.done_p50_ms, Some(2_600));
        assert_eq!(stats.done_p95_ms, Some(4_900));
        assert_eq!(stats.first_p50_ms, Some(260));
        assert_eq!(metrics.finals.len(), WINDOW_SIZE);
    }

    #[test]
    fn joined_ends_replace_absorbed_pending_lag_and_terminals_remove_pending() {
        let mut metrics = Metrics::new();
        metrics.record(&asr(1, 1_000, TextClass::Chinese));
        metrics.record(&asr(2, 2_000, TextClass::Mixed));
        metrics.record(&asr(3, 500, TextClass::English));
        assert_eq!(
            metrics
                .snapshot(StreamTime::from_millis(3_000), 2, 1)
                .lag_ms,
            2_000
        );
        metrics.record(&PipelineEvent::Joined {
            id: UtteranceId(1),
            absorbed: vec![UtteranceId(2)],
            text: "我们，今天。".into(),
            kind: JoinKind::Hold,
        });
        assert_eq!(metrics.unfinished.len(), 1);
        assert_eq!(
            metrics
                .snapshot(StreamTime::from_millis(3_000), 1, 0)
                .lag_ms,
            1_000
        );
        metrics.record(&final_event(1, 300, Some(100)));
        assert_eq!(
            metrics
                .snapshot(StreamTime::from_millis(5_000), 0, 0)
                .lag_ms,
            0
        );
        metrics.record(&asr(4, 3_000, TextClass::Chinese));
        metrics.record(&PipelineEvent::Skipped {
            id: UtteranceId(4),
            reason: SkipReason::CatchUp,
        });
        metrics.record(&asr(5, 4_000, TextClass::Chinese));
        metrics.record(&PipelineEvent::TranslationFailed {
            id: UtteranceId(5),
            reason: FailReason::Timeout,
            message: "too long".into(),
        });
        metrics.record(&asr(6, 5_000, TextClass::Chinese));
        metrics.record(&PipelineEvent::Dropped {
            id: UtteranceId(6),
            reason: DropReason::Empty,
        });
        let stats = metrics.snapshot(StreamTime::from_millis(8_000), 0, 0);
        assert_eq!(stats.skipped_total, 1);
        assert_eq!(stats.failed_total, 1);
        assert_eq!(stats.lag_ms, 0);
    }

    #[test]
    fn invalid_timestamp_order_saturates_and_missing_first_token_stays_absent() {
        let mut metrics = Metrics::new();
        metrics.record(&PipelineEvent::TranslationFinal {
            id: UtteranceId(1),
            text: "the胖est中文".into(),
            timing: Timing {
                speech_end_ms: 1_000,
                done_ms: 900,
                first_token_ms: None,
                ..Timing::default()
            },
        });
        let stats = metrics.snapshot(StreamTime::ZERO, 0, 0);
        assert_eq!(stats.done_p50_ms, Some(0));
        assert_eq!(stats.first_p50_ms, None);
        assert_eq!(metrics.leaked_cjk_total(), 3);
        metrics.record(&final_event(2, u64::from(u32::MAX) + 10, Some(10)));
        assert_eq!(
            metrics.snapshot(StreamTime::ZERO, 0, 0).done_p95_ms,
            Some(u32::MAX)
        );
    }

    #[test]
    fn resource_sampler_is_injected_and_counts_saturate() {
        struct FakeSampler;
        impl ResourceSampler for FakeSampler {
            fn sample(&mut self) -> ResourceSample {
                ResourceSample {
                    cpu_app_pct: 2.5,
                    cpu_translator_pct: 150.0,
                    rss_app_mb: 50,
                    rss_translator_mb: 1_300,
                }
            }
        }
        let mut metrics = Metrics::with_sampler(Box::new(FakeSampler));
        metrics.failed_total = u32::MAX;
        metrics.skipped_total = u32::MAX;
        metrics.record(&PipelineEvent::Skipped {
            id: UtteranceId(1),
            reason: SkipReason::CatchUp,
        });
        metrics.record(&PipelineEvent::TranslationFailed {
            id: UtteranceId(2),
            reason: FailReason::Timeout,
            message: String::new(),
        });
        let stats = metrics.snapshot(StreamTime::ZERO, 0, 0);
        assert_eq!(stats.cpu_app_pct, 2.5);
        assert_eq!(stats.cpu_translator_pct, 150.0);
        assert_eq!(stats.rss_app_mb, 50);
        assert_eq!(stats.rss_translator_mb, 1_300);
        assert_eq!(stats.failed_total, u32::MAX);
        assert_eq!(stats.skipped_total, u32::MAX);
        metrics.set_sampler(None);
        assert_eq!(metrics.snapshot(StreamTime::ZERO, 0, 0).rss_app_mb, 0);
    }

    #[test]
    fn enabled_other_languages_remain_unfinished_until_translation_terminal() {
        let mut metrics = Metrics::with_routing(&["Ja_jp".into(), "Korean".into()]);
        for (index, lang) in ["<|JA|>", "jpn", "ja-JP", "ko_KR", "KOREAN"]
            .iter()
            .enumerate()
        {
            let id = index as u64 + 1;
            let mut event = asr(id, 2_000, TextClass::Other);
            if let PipelineEvent::AsrFinal { lang: tag, .. } = &mut event {
                *tag = Some((*lang).into());
            }
            metrics.record(&event);
            assert_eq!(
                metrics
                    .snapshot(StreamTime::from_millis(3_000), 1, 0)
                    .lag_ms,
                1_000,
                "{lang}"
            );
            metrics.record(&final_event(id, 1_500, Some(1_100)));
            assert_eq!(
                metrics
                    .snapshot(StreamTime::from_millis(3_000), 0, 0)
                    .lag_ms,
                0
            );
        }
        metrics.set_translate_other(&[]);
        metrics.segment_closed(UtteranceId(10), StreamTime::from_millis(1_000));
        let mut event = asr(10, 1_000, TextClass::Other);
        if let PipelineEvent::AsrFinal { lang, .. } = &mut event {
            *lang = Some("ja".into());
        }
        metrics.record(&event);
        assert_eq!(
            metrics
                .snapshot(StreamTime::from_millis(3_000), 0, 0)
                .lag_ms,
            0
        );
    }

    #[test]
    fn closed_segments_track_asr_wait_then_join_and_terminal_without_duplicate_lag() {
        let mut metrics = Metrics::new();
        let live = StreamTime::from_millis(3_000);
        metrics.segment_closed(UtteranceId(1), StreamTime::from_millis(1_000));
        assert_eq!(metrics.snapshot(live, 0, 0).lag_ms, 2_000);
        metrics.record(&asr(1, 1_000, TextClass::Chinese));
        metrics.segment_closed(UtteranceId(2), StreamTime::from_millis(2_000));
        metrics.record(&asr(2, 2_000, TextClass::Chinese));
        assert_eq!(metrics.snapshot(live, 0, 1).lag_ms, 2_000);
        metrics.record(&PipelineEvent::Joined {
            id: UtteranceId(1),
            absorbed: vec![UtteranceId(2)],
            text: "我们，今天。".into(),
            kind: JoinKind::Hold,
        });
        assert_eq!(metrics.snapshot(live, 1, 0).lag_ms, 1_000);
        metrics.record(&final_event(1, 1_500, Some(1_100)));
        assert_eq!(metrics.snapshot(live, 0, 0).lag_ms, 0);
        for (id, class) in [(3, TextClass::English), (4, TextClass::Other)] {
            metrics.segment_closed(UtteranceId(id), StreamTime::from_millis(2_500));
            assert_eq!(metrics.snapshot(live, 0, 0).lag_ms, 500);
            metrics.record(&asr(id, 2_500, class));
            assert_eq!(metrics.snapshot(live, 0, 0).lag_ms, 0);
        }
        metrics.segment_closed(UtteranceId(5), StreamTime::from_millis(2_500));
        metrics.record(&PipelineEvent::Dropped {
            id: UtteranceId(5),
            reason: DropReason::Empty,
        });
        assert_eq!(metrics.snapshot(live, 0, 0).lag_ms, 0);
    }
}
