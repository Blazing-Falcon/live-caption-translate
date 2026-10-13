//! The replay metrics: per-word delay from the moment a word is spoken to the moment the
//! English covering it is on screen, plus counts.
//!
//! Pure functions over a timed event log and a list of speech instants, so the metric code is
//! tested on hand-made logs.

use lt_core::{events::PipelineEvent, types::UtteranceId};
use serde::Serialize;
use std::collections::BTreeMap;

pub struct TimedEvent {
    /// Session time in milliseconds when the event was published.
    pub t_ms: u64,
    pub event: PipelineEvent,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Percentiles {
    pub p50: Option<f64>,
    pub p90: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub struct WordMetrics {
    pub instants: usize,
    /// Seconds from the instant to the final English covering it.
    pub final_english: Percentiles,
    pub uncovered_final: usize,
    pub clauses_by_cut: BTreeMap<String, u64>,
    pub finals_skipped: u64,
    pub cpu_cores_app: Option<f64>,
    pub cpu_cores_final: Option<f64>,
    pub cpu_cores_total: Option<f64>,
    pub system_cpu_pct: Option<f64>,
}

#[derive(Default)]
struct Track {
    start_ms: Option<u64>,
    end_ms: u64,
    final_at: Option<u64>,
    absorbed: Vec<UtteranceId>,
}

fn percentile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = q / 100.0 * (sorted.len() - 1) as f64;
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    Some(sorted[low] + (sorted[high] - sorted[low]) * (rank - low as f64))
}

fn percentiles(mut values: Vec<f64>) -> Percentiles {
    values.sort_by(f64::total_cmp);
    Percentiles {
        p50: percentile(&values, 50.0).map(round2),
        p90: percentile(&values, 90.0).map(round2),
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub fn compute(events: &[TimedEvent], instants: &[u64]) -> WordMetrics {
    let mut tracks: BTreeMap<UtteranceId, Track> = BTreeMap::new();
    let mut metrics = WordMetrics {
        instants: instants.len(),
        ..WordMetrics::default()
    };
    let (mut app, mut fin, mut system, mut stat_count) = (0.0, 0.0, 0.0, 0.0);
    for timed in events {
        match &timed.event {
            PipelineEvent::AsrFinal {
                id,
                start_ms,
                end_ms,
                cut,
                ..
            } => {
                let track = tracks.entry(*id).or_default();
                track.start_ms = Some(*start_ms);
                track.end_ms = *end_ms;
                *metrics
                    .clauses_by_cut
                    .entry(format!("{cut:?}").to_lowercase())
                    .or_default() += 1;
            }
            PipelineEvent::Joined { id, absorbed, .. } => {
                tracks.entry(*id).or_default().absorbed.extend(absorbed);
            }
            PipelineEvent::TranslationFinal { id, .. } => {
                tracks.entry(*id).or_default().final_at = Some(timed.t_ms);
            }
            PipelineEvent::Skipped { .. } => metrics.finals_skipped += 1,
            PipelineEvent::Stats(stats) => {
                app += f64::from(stats.cpu_app_pct);
                fin += f64::from(stats.cpu_translator_pct);
                system += f64::from(stats.cpu_system_pct);
                stat_count += 1.0;
            }
            _ => {}
        }
    }
    if stat_count > 0.0 {
        let cores = |pct: f64| Some(round2(pct / stat_count / 100.0));
        metrics.cpu_cores_app = cores(app);
        metrics.cpu_cores_final = cores(fin);
        metrics.cpu_cores_total = cores(app + fin);
        metrics.system_cpu_pct = Some(round2(system / stat_count));
    }

    // Coverage: (from_ms, to_ms, available_at_ms). A joined line covers its parts.
    let mut cover: Vec<(u64, u64, u64)> = Vec::new();
    for (id, track) in &tracks {
        let Some(done) = track.final_at else {
            continue;
        };
        for member in std::iter::once(id).chain(&track.absorbed) {
            if let Some(part) = tracks.get(member) {
                if let Some(start) = part.start_ms {
                    cover.push((start, part.end_ms, done));
                }
            }
        }
    }
    let mut delays = Vec::new();
    for &t in instants {
        match cover
            .iter()
            .filter(|(from, to, _)| *from <= t && t < *to)
            .map(|(_, _, available)| *available)
            .min()
        {
            Some(available) => delays.push(available.saturating_sub(t) as f64 / 1_000.0),
            None => metrics.uncovered_final += 1,
        }
    }
    metrics.final_english = percentiles(delays);
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::types::{CutReason, TextClass, Timing};

    fn at(t_ms: u64, event: PipelineEvent) -> TimedEvent {
        TimedEvent { t_ms, event }
    }

    fn asr_final(n: u64, start_ms: u64, end_ms: u64, cut: CutReason) -> PipelineEvent {
        PipelineEvent::AsrFinal {
            id: UtteranceId(n),
            text: "你好".into(),
            class: TextClass::Chinese,
            lang: Some("zh".into()),
            start_ms,
            end_ms,
            asr_ms: 100,
            cut,
            logprob: None,
            second_pass: None,
        }
    }

    fn translation_final(n: u64) -> PipelineEvent {
        PipelineEvent::TranslationFinal {
            id: UtteranceId(n),
            text: "So far.".into(),
            timing: Timing::default(),
        }
    }

    #[test]
    fn delays_run_from_each_instant_to_the_final_that_covers_it() {
        // Clause 1 covers 1000..2000 and its final lands at t=3000. Clause 2 (2000..2600) is
        // joined into clause 1. An instant at 2700 is not covered.
        let log = vec![
            at(2_400, asr_final(1, 1_000, 2_000, CutReason::Commit)),
            at(2_700, asr_final(2, 2_000, 2_600, CutReason::Pause)),
            at(
                2_700,
                PipelineEvent::Joined {
                    id: UtteranceId(1),
                    absorbed: vec![UtteranceId(2)],
                    text: "你好你好".into(),
                },
            ),
            at(3_000, translation_final(1)),
        ];
        let metrics = compute(&log, &[1_000, 1_400, 1_900, 2_100, 2_700]);
        // 2000 1600 1100 900 -> sorted 0.9 1.1 1.6 2.0 -> p50 1.35
        assert_eq!(metrics.final_english.p50, Some(1.35));
        assert_eq!(metrics.uncovered_final, 1);
        assert_eq!(metrics.clauses_by_cut.get("commit"), Some(&1));
    }
}
