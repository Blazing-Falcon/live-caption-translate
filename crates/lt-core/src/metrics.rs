//! Event-derived metrics and injected process resource sampling.
use crate::events::PipelineEvent;
use crate::text::chinese_chars;
use crate::types::{PipelineStats, StreamTime, UtteranceId};
use std::collections::{BTreeMap, VecDeque};

const WINDOW_SIZE: usize = 50;
/// Per-word delay medians cover clauses that finished within this long.
const WORD_WINDOW_MS: u64 = 60_000;
/// Instants sampled inside each clause (an approximation of when words are spoken).
const WORD_STEP_MS: u64 = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ResourceSample {
    pub cpu_app_pct: f32,
    pub cpu_translator_pct: f32,
    pub rss_app_mb: u32,
    pub rss_translator_mb: u32,
    /// Whole-PC CPU busy percent over the last second.
    pub cpu_system_pct: f32,
}

/// The shell/CLI supplies OS-specific process counters; core stays portable.
pub trait ResourceSampler: Send {
    fn sample(&mut self) -> ResourceSample;
}

struct FinalSample {
    done_ms: u32,
    first_ms: Option<u32>,
}

/// What is known about a clause until its final translation finishes.
#[derive(Default)]
struct ClauseTrack {
    start_ms: u64,
    end_ms: u64,
}

/// Per-word delays of one finished clause, in milliseconds.
struct WordClause {
    done_ms: u64,
    delays: Vec<u32>,
}

#[derive(Default)]
pub struct Metrics {
    finals: VecDeque<FinalSample>,
    unfinished: BTreeMap<UtteranceId, u64>,
    skipped_total: u32,
    failed_total: u32,
    leaked_cjk_total: u64,
    sampler: Option<Box<dyn ResourceSampler>>,
    tracks: BTreeMap<UtteranceId, ClauseTrack>,
    words: VecDeque<WordClause>,
    resources: ResourceSample,
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

    /// How far the oldest unfinished clause trails the stream, in milliseconds.
    pub fn lag_ms(&self, live: StreamTime) -> u64 {
        self.unfinished
            .values()
            .min()
            .map_or(0, |end| live.millis().saturating_sub(*end))
    }

    fn forget_clause(&mut self, id: &UtteranceId) {
        self.tracks.remove(id);
    }

    /// Per-word delay samples for a finished clause, from each instant to the final.
    fn finish_words(&mut self, id: UtteranceId, done_ms: u64) {
        let Some(track) = self.tracks.remove(&id) else {
            return;
        };
        let mut clause = WordClause {
            done_ms,
            delays: Vec::new(),
        };
        let mut t = track.start_ms;
        while t < track.end_ms {
            clause
                .delays
                .push(milliseconds_u32(done_ms.saturating_sub(t)));
            t += WORD_STEP_MS;
        }
        self.words.push_back(clause);
        while self
            .words
            .front()
            .is_some_and(|front| front.done_ms + WORD_WINDOW_MS < done_ms)
        {
            self.words.pop_front();
        }
    }

    pub fn record(&mut self, event: &PipelineEvent) {
        self.record_event(event);
        match event {
            PipelineEvent::AsrFinal {
                id,
                start_ms,
                end_ms,
                ..
            } => {
                self.tracks.insert(
                    *id,
                    ClauseTrack {
                        start_ms: *start_ms,
                        end_ms: *end_ms,
                    },
                );
            }
            PipelineEvent::Joined { id, absorbed, .. } => {
                for gone in absorbed {
                    if let Some(track) = self.tracks.remove(gone) {
                        let leader = self.tracks.entry(*id).or_default();
                        if leader.end_ms == 0 {
                            leader.start_ms = track.start_ms;
                        }
                        leader.start_ms = leader.start_ms.min(track.start_ms);
                        leader.end_ms = leader.end_ms.max(track.end_ms);
                    }
                }
            }
            PipelineEvent::TranslationFinal { id, timing, .. } => {
                self.finish_words(*id, timing.done_ms);
            }
            PipelineEvent::Skipped { id, .. }
            | PipelineEvent::TranslationFailed { id, .. }
            | PipelineEvent::Dropped { id, .. } => self.forget_clause(id),
            _ => {}
        }
    }

    fn record_event(&mut self, event: &PipelineEvent) {
        match event {
            PipelineEvent::AsrFinal {
                id, class, end_ms, ..
            } => {
                if class.is_translated() {
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
    pub fn snapshot(&mut self, live: StreamTime, queue_depth: u32) -> PipelineStats {
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
        self.resources = resources;
        let word_delays: Vec<u32> = self
            .words
            .iter()
            .flat_map(|clause| clause.delays.iter().copied())
            .collect();
        PipelineStats {
            word_final_p50_ms: nearest_rank(&word_delays, 50),
            cpu_system_pct: resources.cpu_system_pct,
            ..self.base_snapshot(live, queue_depth, &done, &first, resources)
        }
    }

    fn base_snapshot(
        &self,
        live: StreamTime,
        queue_depth: u32,
        done: &[u32],
        first: &[u32],
        resources: ResourceSample,
    ) -> PipelineStats {
        PipelineStats {
            lag_ms: self
                .unfinished
                .values()
                .min()
                .map_or(0, |end| live.millis().saturating_sub(*end)),
            queue_depth,
            done_p50_ms: nearest_rank(done, 50),
            done_p95_ms: nearest_rank(done, 95),
            first_p50_ms: nearest_rank(first, 50),
            skipped_total: self.skipped_total,
            failed_total: self.failed_total,
            cpu_app_pct: resources.cpu_app_pct,
            cpu_translator_pct: resources.cpu_translator_pct,
            rss_app_mb: resources.rss_app_mb,
            rss_translator_mb: resources.rss_translator_mb,
            ..PipelineStats::default()
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
    use crate::types::{TextClass, Timing};

    fn asr(id: u64, end: u64, class: TextClass) -> PipelineEvent {
        PipelineEvent::AsrFinal {
            id: UtteranceId(id),
            text: "我们。".into(),
            class,
            lang: Some("zh".into()),
            start_ms: end.saturating_sub(500),
            end_ms: end,
            asr_ms: 100,
            cut: crate::types::CutReason::Pause,
            logprob: None,
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
    fn per_word_delays_run_from_each_instant_to_the_final() {
        let mut metrics = Metrics::new();
        // Clause 1 spans 1000..1500 ms; instants at 1000, 1100, 1200, 1300, 1400.
        metrics.record(&asr(1, 1_500, TextClass::Chinese));
        // The final finishes at 3000.
        metrics.record(&final_event_at(1, 3_000));
        let stats = metrics.snapshot(StreamTime::from_millis(3_000), 0);
        // final English per instant: 2000 1900 1800 1700 1600 -> median 1800
        assert_eq!(stats.word_final_p50_ms, Some(1_800));
    }

    fn final_event_at(id: u64, done_ms: u64) -> PipelineEvent {
        PipelineEvent::TranslationFinal {
            id: UtteranceId(id),
            text: "We are here.".into(),
            timing: Timing {
                speech_end_ms: 0,
                done_ms,
                ..Timing::default()
            },
        }
    }

    #[test]
    fn percentile_window_uses_nearest_rank_over_last_fifty_finals() {
        let mut metrics = Metrics::new();
        assert_eq!(
            metrics.snapshot(StreamTime::ZERO, 0),
            PipelineStats::default()
        );
        for index in 1..=50 {
            metrics.record(&final_event(index, index * 100, Some(index * 10)));
        }
        let stats = metrics.snapshot(StreamTime::ZERO, 3);
        assert_eq!(stats.done_p50_ms, Some(2_500));
        assert_eq!(stats.done_p95_ms, Some(4_800));
        assert_eq!(stats.first_p50_ms, Some(250));
        assert_eq!(stats.queue_depth, 3);
        metrics.record(&final_event(51, 9_000, None));
        let stats = metrics.snapshot(StreamTime::ZERO, 0);
        assert_eq!(stats.done_p50_ms, Some(2_600));
        assert_eq!(stats.done_p95_ms, Some(4_900));
        assert_eq!(stats.first_p50_ms, Some(260));
        assert_eq!(metrics.finals.len(), WINDOW_SIZE);
    }
}
