//! The replay metrics: per-word delay from the moment a
//! word is spoken to the moment English covering it is on screen, plus rewrites and counts.
//!
//! Pure functions over a timed event log and a list of speech instants, so the metric code is
//! tested on hand-made logs.

use lt_core::{
    draft::{visible_words, DisplayPolicy},
    engines::Vad,
    events::PipelineEvent,
    types::{EffectiveMode, UtteranceId},
};
use serde::Serialize;
use std::collections::BTreeMap;

/// Frames of the pipeline's VAD input.
const FRAME_SAMPLES: usize = 512;
/// One instant every third 32 ms frame (about 0.1 s), as the Python kit does.
const INSTANT_EVERY_FRAMES: usize = 3;
const SPEECH_PROBABILITY: f32 = 0.5;

pub struct TimedEvent {
    /// Session time in milliseconds when the event was published.
    pub t_ms: u64,
    pub event: PipelineEvent,
}

/// Speech instants (stream milliseconds) of 16 kHz audio, using the pipeline's VAD.
pub fn speech_instants(samples: &[f32], vad: &mut dyn Vad) -> Vec<u64> {
    let mut instants = Vec::new();
    for (index, frame) in samples.as_chunks::<FRAME_SAMPLES>().0.iter().enumerate() {
        let probability = vad.speech_prob(frame);
        if index % INSTANT_EVERY_FRAMES == 0 && probability >= SPEECH_PROBABILITY {
            instants.push((index * FRAME_SAMPLES) as u64 * 1_000 / 16_000);
        }
    }
    instants
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Percentiles {
    pub p50: Option<f64>,
    pub p90: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub struct WordMetrics {
    pub instants: usize,
    /// Seconds from the instant to the first draft or final that covers it.
    pub first: Percentiles,
    /// Like `first`, but a draft covers only up to the previous draft's end (hold back).
    pub first_shown: Percentiles,
    /// Seconds from the instant to the final English covering it.
    pub final_english: Percentiles,
    pub uncovered_first: usize,
    pub uncovered_final: usize,
    /// Words removed per final word, raw drafts and with the `hold2` display policy.
    pub rewrites_raw: Option<f64>,
    pub rewrites_hold2: Option<f64>,
    pub clauses_by_cut: BTreeMap<String, u64>,
    pub drafts_shown: u64,
    pub drafts_failed: u32,
    pub finals_skipped: u64,
    pub step_downs: u64,
    pub cpu_cores_app: Option<f64>,
    pub cpu_cores_final: Option<f64>,
    pub cpu_cores_draft: Option<f64>,
    pub cpu_cores_total: Option<f64>,
    pub system_cpu_pct: Option<f64>,
}

#[derive(Default)]
struct Track {
    start_ms: Option<u64>,
    end_ms: u64,
    spoken_at: Option<u64>,
    /// `(published t_ms, end_ms of the translated partial, text)`.
    drafts: Vec<(u64, u64, String)>,
    final_at: Option<u64>,
    final_text: String,
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

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

/// Words of `previous` that are not a common prefix of `next`.
fn removed_words(previous: &[String], next: &[String]) -> usize {
    let common = previous
        .iter()
        .zip(next)
        .take_while(|(a, b)| a == b)
        .count();
    previous.len() - common
}

fn rewrites(sequences: &[Vec<Vec<String>>], final_words: usize) -> Option<f64> {
    if final_words == 0 {
        return None;
    }
    let removed: usize = sequences
        .iter()
        .map(|texts| {
            texts
                .windows(2)
                .map(|pair| removed_words(&pair[0], &pair[1]))
                .sum::<usize>()
        })
        .sum();
    Some(round2(removed as f64 / final_words as f64))
}

pub fn compute(events: &[TimedEvent], instants: &[u64]) -> WordMetrics {
    let mut tracks: BTreeMap<UtteranceId, Track> = BTreeMap::new();
    let mut metrics = WordMetrics {
        instants: instants.len(),
        ..WordMetrics::default()
    };
    let (mut app, mut fin, mut draft, mut system, mut stat_count) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut last_mode = None;
    for timed in events {
        match &timed.event {
            PipelineEvent::SpeechStarted { id, at_ms } => {
                tracks.entry(*id).or_default().spoken_at = Some(*at_ms);
            }
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
            PipelineEvent::TranslationDraft {
                id, end_ms, text, ..
            } => {
                metrics.drafts_shown += 1;
                tracks
                    .entry(*id)
                    .or_default()
                    .drafts
                    .push((timed.t_ms, *end_ms, text.clone()));
            }
            PipelineEvent::Joined { id, absorbed, .. } => {
                tracks.entry(*id).or_default().absorbed.extend(absorbed);
            }
            PipelineEvent::TranslationFinal { id, text, .. } => {
                let track = tracks.entry(*id).or_default();
                track.final_at = Some(timed.t_ms);
                track.final_text.clone_from(text);
            }
            PipelineEvent::Skipped { .. } => metrics.finals_skipped += 1,
            PipelineEvent::Stats(stats) => {
                app += f64::from(stats.cpu_app_pct);
                fin += f64::from(stats.cpu_translator_pct);
                draft += f64::from(stats.cpu_draft_pct);
                system += f64::from(stats.cpu_system_pct);
                stat_count += 1.0;
                metrics.drafts_failed = stats.drafts_failed;
                if last_mode == Some(EffectiveMode::Continuous)
                    && stats.mode == EffectiveMode::Light
                {
                    metrics.step_downs += 1;
                }
                last_mode = Some(stats.mode);
            }
            _ => {}
        }
    }
    if stat_count > 0.0 {
        let cores = |pct: f64| Some(round2(pct / stat_count / 100.0));
        metrics.cpu_cores_app = cores(app);
        metrics.cpu_cores_final = cores(fin);
        metrics.cpu_cores_draft = cores(draft);
        metrics.cpu_cores_total = cores(app + fin + draft);
        metrics.system_cpu_pct = Some(round2(system / stat_count));
    }

    // Coverage: (from_ms, to_ms, available_at_ms)
    let mut first_cover: Vec<(u64, u64, u64)> = Vec::new();
    let mut shown_cover: Vec<(u64, u64, u64)> = Vec::new();
    let mut final_cover: Vec<(u64, u64, u64)> = Vec::new();
    for track in tracks.values() {
        let Some(from) = track.start_ms.or(track.spoken_at) else {
            continue;
        };
        let mut previous_end = None;
        for (published, end_ms, _) in &track.drafts {
            first_cover.push((from, *end_ms, *published));
            if let Some(held_to) = previous_end {
                shown_cover.push((from, held_to, *published));
            }
            previous_end = Some(*end_ms);
        }
    }
    for (id, track) in &tracks {
        let Some(done) = track.final_at else {
            continue;
        };
        let mut members = vec![*id];
        members.extend(&track.absorbed);
        for member in members {
            if let Some(t) = tracks.get(&member) {
                if let Some(start) = t.start_ms {
                    final_cover.push((start, t.end_ms, done));
                    first_cover.push((start, t.end_ms, done));
                    shown_cover.push((start, t.end_ms, done));
                }
            }
        }
    }
    let delays = |cover: &[(u64, u64, u64)]| -> (Vec<f64>, usize) {
        let mut values = Vec::new();
        let mut uncovered = 0;
        for &t in instants {
            match cover
                .iter()
                .filter(|(from, to, _)| *from <= t && t < *to)
                .map(|(_, _, available)| *available)
                .min()
            {
                Some(available) => values.push(available.saturating_sub(t) as f64 / 1_000.0),
                None => uncovered += 1,
            }
        }
        (values, uncovered)
    };
    let (first, uncovered_first) = delays(&first_cover);
    let (shown, _) = delays(&shown_cover);
    let (last, uncovered_final) = delays(&final_cover);
    metrics.first = percentiles(first);
    metrics.first_shown = percentiles(shown);
    metrics.final_english = percentiles(last);
    metrics.uncovered_first = uncovered_first;
    metrics.uncovered_final = uncovered_final;

    // Rewrites: the displayed English of every clause that got a final of its own.
    let mut raw_sequences = Vec::new();
    let mut hold_sequences = Vec::new();
    let mut final_words = 0;
    for (id, track) in &tracks {
        if track.final_at.is_none() || tracks.values().any(|t| t.absorbed.contains(id)) {
            continue;
        }
        let final_text: Vec<String> = words(&track.final_text)
            .into_iter()
            .map(str::to_owned)
            .collect();
        final_words += final_text.len();
        let mut raw: Vec<Vec<String>> = Vec::new();
        let mut hold: Vec<Vec<String>> = Vec::new();
        let mut drafts: Vec<String> = Vec::new();
        let (mut shown, mut held) = (Vec::new(), false);
        for (_, _, text) in &track.drafts {
            raw.push(words(text).into_iter().map(str::to_owned).collect());
            drafts.push(text.clone());
            let (visible, was_held) =
                visible_words(DisplayPolicy::Hold2, &drafts, false, &shown, held);
            hold.push(visible.clone());
            shown = visible;
            held = was_held;
        }
        raw.push(final_text.clone());
        hold.push(final_text);
        raw_sequences.push(raw);
        hold_sequences.push(hold);
    }
    metrics.rewrites_raw = rewrites(&raw_sequences, final_words);
    metrics.rewrites_hold2 = rewrites(&hold_sequences, final_words);
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::types::{CutReason, TextClass, Timing};

    fn at(t_ms: u64, event: PipelineEvent) -> TimedEvent {
        TimedEvent { t_ms, event }
    }

    fn id(n: u64) -> UtteranceId {
        UtteranceId(n)
    }

    fn asr_final(n: u64, start_ms: u64, end_ms: u64, cut: CutReason) -> PipelineEvent {
        PipelineEvent::AsrFinal {
            id: id(n),
            text: "你好".into(),
            class: TextClass::Chinese,
            lang: Some("zh".into()),
            start_ms,
            end_ms,
            asr_ms: 100,
            cut,
        }
    }

    fn draft(n: u64, rev: u32, text: &str, end_ms: u64) -> PipelineEvent {
        PipelineEvent::TranslationDraft {
            id: id(n),
            rev,
            text: text.into(),
            end_ms,
        }
    }

    fn translation_final(n: u64, text: &str) -> PipelineEvent {
        PipelineEvent::TranslationFinal {
            id: id(n),
            text: text.into(),
            timing: Timing::default(),
        }
    }

    #[test]
    fn delays_use_the_earliest_covering_draft_or_final() {
        // Clause 1 covers 1000..2000. Drafts: up to 1500 at t=1800, up to 2000 at t=2300.
        // The final lands at t=3000.
        let log = vec![
            at(1_800, draft(1, 1, "so far", 1_500)),
            at(2_300, draft(1, 2, "so far, all of it", 2_000)),
            at(2_400, asr_final(1, 1_000, 2_000, CutReason::Commit)),
            at(3_000, translation_final(1, "So far, all of it.")),
        ];
        let instants = [1_000, 1_400, 1_600, 1_900];
        let metrics = compute(&log, &instants);
        // first: 1000->800, 1400->400, 1600->700 (second draft), 1900->400 => sorted 400 400 700 800
        // linear percentile 50 of [0.4,0.4,0.7,0.8] = 0.55
        assert_eq!(metrics.first.p50, Some(0.55));
        // final: 2000 1600 1400 1100 => sorted 1.1 1.4 1.6 2.0 -> p50 1.5
        assert_eq!(metrics.final_english.p50, Some(1.5));
        // shown: the first draft covers nothing; the second covers up to 1500 only
        // 1000->1300 (second draft at 2300), 1400->900, 1600->final 3000-1600=1400, 1900->1100
        // sorted 0.9 1.1 1.3 1.4 -> p50 1.2
        assert_eq!(metrics.first_shown.p50, Some(1.2));
        assert_eq!(metrics.uncovered_first, 0);
        assert_eq!(metrics.drafts_shown, 2);
        assert_eq!(metrics.clauses_by_cut.get("commit"), Some(&1));
    }

    #[test]
    fn rewrites_count_words_removed_between_displayed_texts() {
        let log = vec![
            at(1_000, draft(1, 1, "I want to buy", 1_000)),
            at(1_500, draft(1, 2, "I would like to buy it", 1_400)),
            at(1_600, asr_final(1, 500, 1_400, CutReason::Commit)),
            at(2_000, translation_final(1, "I would like to buy it.")),
        ];
        let metrics = compute(&log, &[600]);
        // raw: "I want to buy" -> "I would like to buy it": common prefix 1 => 3 removed;
        // then -> final "I would like to buy it.": common prefix 5 of 6 => 1 removed. 4 / 6 words
        assert_eq!(metrics.rewrites_raw, Some(0.67));
        // hold2 shows the first max(min(n,2), n-2) words: "I want", then "I would like to"
        // ("I want" -> "I would like to" removes 1: "want"), then the full final removes
        // "to"? no: "I would like to" is a prefix of "I would like to buy it." => 0.
        assert_eq!(metrics.rewrites_hold2, Some(0.17));
    }
}
