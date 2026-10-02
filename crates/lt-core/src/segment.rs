//! Streaming segmentation with bounded frame history and Python-reference arithmetic.
//!
//! The reference rounds speech/silence to frames, but applies pre/post padding in
//! samples. Its final `end` flush deliberately omits pre-roll; pause/cut output
//! keeps it. Hard cuts retain already-read audio after the selected energy frame.

use std::{collections::VecDeque, ops::BitOr, sync::Arc};

use crate::{
    config::VadConfig,
    types::{CutReason, Segment, StreamTime, UtteranceId, FRAME_SAMPLES, SAMPLE_RATE},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameFlags {
    pub gap_filled: bool,
    pub discontinuity: bool,
}

impl FrameFlags {
    pub const EMPTY: Self = Self {
        gap_filled: false,
        discontinuity: false,
    };
    pub const GAP_FILLED: Self = Self {
        gap_filled: true,
        discontinuity: false,
    };
    pub const DISCONTINUITY: Self = Self {
        gap_filled: false,
        discontinuity: true,
    };

    pub const fn contains(self, other: Self) -> bool {
        (!other.gap_filled || self.gap_filled) && (!other.discontinuity || self.discontinuity)
    }
}

impl BitOr for FrameFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self {
            gap_filled: self.gap_filled || other.gap_filled,
            discontinuity: self.discontinuity || other.discontinuity,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SegmentUpdate {
    pub started: Vec<(UtteranceId, StreamTime)>,
    pub segments: Vec<Segment>,
}

struct AudioFrame {
    start: u64,
    samples: [f32; FRAME_SAMPLES],
    energy: f64,
}

struct Speech {
    id: UtteranceId,
    /// First actual speech sample, or the preceding cut boundary for a continuation.
    start: u64,
    last_speech: u64,
    silence: u64,
    pre_roll: bool,
}

struct PendingSegment {
    speech: Speech,
    end: u64,
}

pub struct SegmentBuilder {
    config: VadConfig,
    min_speech_frames: u64,
    min_silence_frames: u64,
    pre_samples: u64,
    post_samples: u64,
    soft_samples: u64,
    hard_samples: u64,
    current: u64,
    epoch_start: u64,
    next_id: u64,
    speech_run: u64,
    speech: Option<Speech>,
    /// Only pre-roll/open-segment audio is kept; emitted stream history is discarded.
    history: VecDeque<AudioFrame>,
    /// Used only when configured post-roll exceeds the silence needed to close.
    pending: VecDeque<PendingSegment>,
}

impl SegmentBuilder {
    /// The pipeline supplies a validated config (hard cuts are bounded to 20 s).
    pub fn new(config: VadConfig) -> Self {
        Self {
            min_speech_frames: python_frames(config.min_speech_s).max(1),
            min_silence_frames: python_frames(config.min_silence_s).max(1),
            pre_samples: sample_duration(config.pre_roll_s),
            post_samples: sample_duration(config.post_roll_s),
            soft_samples: sample_duration(config.soft_cut_after_s),
            hard_samples: sample_duration(config.hard_cut_s),
            config,
            current: 0,
            epoch_start: 0,
            next_id: 0,
            speech_run: 0,
            speech: None,
            history: VecDeque::new(),
            pending: VecDeque::new(),
        }
    }

    pub fn current_time(&self) -> StreamTime {
        StreamTime(self.current)
    }

    /// Close before a source clock jump, without synthesizing the skipped audio.
    /// Pre-roll and frame history never cross this boundary; ids keep increasing.
    pub fn discontinuity(&mut self, at: StreamTime) -> SegmentUpdate {
        let mut update = SegmentUpdate::default();
        self.flush_pending(true, &mut update);
        if let Some(speech) = self.speech.take() {
            self.emit(&speech, self.current, CutReason::Discontinuity, &mut update);
        }
        self.history.clear();
        self.speech_run = 0;
        self.current = self.current.max(at.0);
        self.epoch_start = self.current;
        update
    }

    pub fn push(
        &mut self,
        samples: &[f32; FRAME_SAMPLES],
        probability: f32,
        flags: FrameFlags,
    ) -> SegmentUpdate {
        let mut update = if flags.discontinuity {
            self.discontinuity(self.current_time())
        } else {
            SegmentUpdate::default()
        };
        let frame_start = self.current;
        let energy = samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / FRAME_SAMPLES as f64;
        self.history.push_back(AudioFrame {
            start: frame_start,
            samples: *samples,
            energy,
        });
        self.current = self.current.saturating_add(FRAME_SAMPLES as u64);
        self.flush_pending(false, &mut update);

        // Filled packet holes are silence even if a recurrent VAD retains a high probability.
        let probability = if flags.gap_filled || !probability.is_finite() {
            0.0
        } else {
            probability
        };
        match self.speech.take() {
            None => {
                self.speech_run = if probability >= self.config.threshold {
                    self.speech_run.saturating_add(1)
                } else {
                    0
                };
                if self.speech_run >= self.min_speech_frames {
                    let start = frame_start
                        .saturating_sub((self.speech_run - 1).saturating_mul(FRAME_SAMPLES as u64));
                    let speech = Speech {
                        id: self.allocate_id(),
                        start,
                        last_speech: frame_start,
                        silence: 0,
                        pre_roll: true,
                    };
                    update.started.push((speech.id, StreamTime(start)));
                    self.speech = Some(speech);
                }
            }
            Some(mut speech) => {
                if probability >= self.config.threshold {
                    speech.last_speech = frame_start;
                    speech.silence = 0;
                } else {
                    speech.silence = speech.silence.saturating_add(1);
                }
                let duration = self.current.saturating_sub(speech.start);
                if speech.silence >= self.min_silence_frames {
                    let end = speech
                        .last_speech
                        .saturating_add(FRAME_SAMPLES as u64)
                        .saturating_add(self.post_samples);
                    if end <= self.current {
                        self.emit(&speech, end, CutReason::Pause, &mut update);
                    } else {
                        self.pending.push_back(PendingSegment { speech, end });
                    }
                    self.speech_run = 0;
                } else if duration >= self.soft_samples && probability < self.config.soft_cut_prob {
                    self.emit(&speech, self.current, CutReason::SoftCut, &mut update);
                    speech.id = self.allocate_id();
                    speech.start = self.current;
                    speech.pre_roll = false;
                    // Reference arithmetic keeps the preceding breath's silence count.
                    speech.last_speech = frame_start;
                    update.started.push((speech.id, StreamTime(speech.start)));
                    self.speech = Some(speech);
                } else if duration >= self.hard_samples {
                    let cut = self.hard_cut_boundary(&speech, frame_start);
                    self.emit(&speech, cut, CutReason::HardCut, &mut update);
                    speech.id = self.allocate_id();
                    speech.start = cut;
                    speech.pre_roll = false;
                    speech.last_speech = speech.last_speech.max(cut);
                    update.started.push((speech.id, StreamTime(cut)));
                    self.speech = Some(speech);
                } else {
                    self.speech = Some(speech);
                }
            }
        }
        self.trim_history();
        update
    }

    /// Flush a finite replay. Like the Python reference, End starts at speech onset.
    pub fn finish(&mut self) -> SegmentUpdate {
        let mut update = SegmentUpdate::default();
        self.flush_pending(true, &mut update);
        if let Some(mut speech) = self.speech.take() {
            speech.pre_roll = false;
            self.emit(&speech, self.current, CutReason::End, &mut update);
        }
        self.history.clear();
        self.speech_run = 0;
        self.epoch_start = self.current;
        update
    }

    fn allocate_id(&mut self) -> UtteranceId {
        let id = UtteranceId(self.next_id);
        // A session cannot approach u64 exhaustion in a practical lifetime.
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn start_sample(&self, speech: &Speech) -> u64 {
        if speech.pre_roll {
            speech
                .start
                .saturating_sub(self.pre_samples)
                .max(self.epoch_start)
        } else {
            speech.start
        }
    }

    fn emit(&self, speech: &Speech, end: u64, reason: CutReason, update: &mut SegmentUpdate) {
        let start = self.start_sample(speech);
        let end = end.min(self.current);
        if end <= start {
            return;
        }
        let mut pcm = Vec::with_capacity((end - start) as usize);
        for frame in &self.history {
            let frame_end = frame.start.saturating_add(FRAME_SAMPLES as u64);
            if frame_end <= start || frame.start >= end {
                continue;
            }
            let first = start.saturating_sub(frame.start) as usize;
            let last = (end.min(frame_end) - frame.start) as usize;
            pcm.extend_from_slice(&frame.samples[first..last]);
        }
        update.segments.push(Segment {
            id: speech.id,
            start: StreamTime(start),
            end: StreamTime(end),
            samples: Arc::from(pcm),
            cut_reason: reason,
        });
    }

    fn hard_cut_boundary(&self, speech: &Speech, frame_start: u64) -> u64 {
        // Python includes both ends of [i - round(1 / .032), i]: 32 frames.
        let lookback = python_frames(1.0).saturating_mul(FRAME_SAMPLES as u64);
        let lower = frame_start.saturating_sub(lookback).max(speech.start);
        let mut minimum = f64::INFINITY;
        let mut cut = lower;
        for frame in &self.history {
            if frame.start >= lower && frame.start <= frame_start && frame.energy < minimum {
                minimum = frame.energy;
                cut = frame.start;
            }
        }
        cut.saturating_add(FRAME_SAMPLES as u64)
    }

    fn flush_pending(&mut self, force: bool, update: &mut SegmentUpdate) {
        while self
            .pending
            .front()
            .is_some_and(|pending| force || pending.end <= self.current)
        {
            if let Some(pending) = self.pending.pop_front() {
                self.emit(&pending.speech, pending.end, CutReason::Pause, update);
            }
        }
    }

    fn trim_history(&mut self) {
        let idle_keep = self
            .pre_samples
            .saturating_add(self.min_speech_frames.saturating_mul(FRAME_SAMPLES as u64));
        let mut earliest = self.current.saturating_sub(idle_keep).max(self.epoch_start);
        if let Some(speech) = &self.speech {
            earliest = earliest.min(self.start_sample(speech));
        }
        for pending in &self.pending {
            earliest = earliest.min(self.start_sample(&pending.speech));
        }
        while self
            .history
            .front()
            .is_some_and(|frame| frame.start.saturating_add(FRAME_SAMPLES as u64) <= earliest)
        {
            self.history.pop_front();
        }
    }
}

fn decimal_seconds(value: f32) -> f64 {
    // Undo f32 representation noise before Python's ties-to-even frame rounding.
    (f64::from(value) * 1_000_000.0).round() / 1_000_000.0
}

fn python_frames(seconds: f32) -> u64 {
    (decimal_seconds(seconds) / (FRAME_SAMPLES as f64 / SAMPLE_RATE as f64)).round_ties_even()
        as u64
}

fn sample_duration(seconds: f32) -> u64 {
    (decimal_seconds(seconds) * SAMPLE_RATE as f64).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn append(into: &mut SegmentUpdate, update: SegmentUpdate) {
        into.started.extend(update.started);
        into.segments.extend(update.segments);
    }

    fn frames(builder: &mut SegmentBuilder, count: usize, probability: f32) -> SegmentUpdate {
        let mut output = SegmentUpdate::default();
        for _ in 0..count {
            append(
                &mut output,
                builder.push(&[1.0; FRAME_SAMPLES], probability, FrameFlags::EMPTY),
            );
        }
        output
    }

    #[test]
    fn frame_rounding_matches_python_ties_even() {
        assert_eq!(python_frames(0.25), 8);
        assert_eq!(python_frames(0.4), 12);
        assert_eq!(python_frames(0.08), 2);
        assert_eq!(python_frames(0.112), 4);
        assert_eq!(sample_duration(0.3), 4_800);
        assert_eq!(sample_duration(0.1), 1_600);
    }

    #[test]
    fn opening_requires_the_full_speech_run_and_exact_padding() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        assert!(frames(&mut builder, 7, 0.9).started.is_empty());
        assert!(frames(&mut builder, 12, 0.0).segments.is_empty());
        assert!(builder.finish().segments.is_empty());

        let mut builder = SegmentBuilder::new(VadConfig::default());
        frames(&mut builder, 20, 0.0);
        let opened = frames(&mut builder, 8, 0.9);
        assert_eq!(opened.started, vec![(UtteranceId(0), StreamTime(20 * 512))]);
        assert!(frames(&mut builder, 11, 0.0).segments.is_empty());
        let closed = frames(&mut builder, 1, 0.0);
        let segment = &closed.segments[0];
        assert_eq!(segment.start, StreamTime(20 * 512 - 4_800));
        assert_eq!(segment.end, StreamTime(28 * 512 + 1_600));
        assert_eq!(
            segment.samples.len() as u64,
            segment.end.0 - segment.start.0
        );
        assert_eq!(segment.cut_reason, CutReason::Pause);
        assert!(builder.finish().segments.is_empty());
    }

    #[test]
    fn initial_pre_roll_clamps_at_zero() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        frames(&mut builder, 8, 0.9);
        let closed = frames(&mut builder, 12, 0.0);
        assert_eq!(closed.segments[0].start, StreamTime::ZERO);
    }

    #[test]
    fn soft_cut_uses_first_breath_and_continues_without_overlap() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        let mut output = frames(&mut builder, 235, 0.9);
        append(
            &mut output,
            builder.push(&[1.0; FRAME_SAMPLES], 0.3, FrameFlags::EMPTY),
        );
        assert_eq!(output.segments.len(), 1);
        assert_eq!(output.segments[0].cut_reason, CutReason::SoftCut);
        assert_eq!(output.segments[0].end, StreamTime(236 * 512));
        assert_eq!(output.started[1], (UtteranceId(1), StreamTime(236 * 512)));
        frames(&mut builder, 45, 0.9);
        let tail = builder.finish();
        assert_eq!(tail.segments[0].start, output.segments[0].end);
        assert_eq!(tail.segments[0].cut_reason, CutReason::End);

        let mut builder = SegmentBuilder::new(VadConfig::default());
        frames(&mut builder, 235, 0.9);
        assert!(builder
            .push(&[1.0; FRAME_SAMPLES], 0.4, FrameFlags::EMPTY)
            .segments
            .is_empty());
        assert_eq!(builder.finish().segments[0].cut_reason, CutReason::End);
    }

    #[test]
    fn hard_cut_chooses_the_lowest_energy_in_the_last_second() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        let mut output = SegmentUpdate::default();
        for index in 0..937 {
            let amplitude = if index == 294 { 0.1 } else { 1.0 };
            append(
                &mut output,
                builder.push(&[amplitude; FRAME_SAMPLES], 0.9, FrameFlags::EMPTY),
            );
        }
        append(&mut output, builder.finish());
        assert_eq!(output.segments[0].end, StreamTime(295 * 512));
        assert_eq!(output.segments[0].cut_reason, CutReason::HardCut);
        for pair in output.segments.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
            assert!(pair[0].id < pair[1].id);
        }
        for segment in output
            .segments
            .iter()
            .filter(|s| s.cut_reason == CutReason::HardCut)
        {
            let duration = segment.end.0 - segment.start.0;
            assert!((9 * SAMPLE_RATE..=10 * SAMPLE_RATE + 512).contains(&duration));
            assert_eq!(segment.samples.len() as u64, duration);
        }
    }

    #[test]
    fn equal_energy_hard_cut_chooses_the_earliest_inclusive_frame() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        let output = frames(&mut builder, 313, 0.9);
        assert_eq!(output.segments[0].end, StreamTime(282 * 512));
        assert_eq!(output.started[1].1, StreamTime(282 * 512));
    }

    #[test]
    fn discontinuity_closes_before_ingress_and_discards_old_pre_roll() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        frames(&mut builder, 10, 0.0);
        frames(&mut builder, 12, 0.9);
        let boundary = builder.current_time();
        let cut = builder.push(&[2.0; FRAME_SAMPLES], 0.9, FrameFlags::DISCONTINUITY);
        assert_eq!(cut.segments[0].end, boundary);
        assert_eq!(cut.segments[0].cut_reason, CutReason::Discontinuity);
        let opened = frames(&mut builder, 7, 0.9);
        assert_eq!(opened.started[0], (UtteranceId(1), boundary));
        let closed = frames(&mut builder, 12, 0.0);
        assert_eq!(closed.segments[0].start, boundary);
        assert_eq!(closed.segments[0].samples[0], 2.0);
    }

    #[test]
    fn clock_jump_advances_without_audio_or_id_reuse() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        frames(&mut builder, 8, 0.9);
        let previous = builder.current_time();
        let jumped = builder.discontinuity(StreamTime::from_seconds(60.0));
        assert_eq!(jumped.segments[0].end, previous);
        assert_eq!(jumped.segments[0].cut_reason, CutReason::Discontinuity);
        assert!(builder.history.is_empty());
        assert_eq!(builder.current_time(), StreamTime::from_seconds(60.0));
        assert_eq!(frames(&mut builder, 8, 0.9).started[0].0, UtteranceId(1));
        let closed = frames(&mut builder, 12, 0.0);
        assert_eq!(closed.segments[0].start, StreamTime::from_seconds(60.0));
        let now = builder.current_time();
        builder.discontinuity(StreamTime::ZERO);
        assert_eq!(builder.current_time(), now);
    }

    #[test]
    fn gap_filled_frames_close_as_silence() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        frames(&mut builder, 8, 0.9);
        for _ in 0..11 {
            assert!(builder
                .push(&[0.0; FRAME_SAMPLES], 0.9, FrameFlags::GAP_FILLED)
                .segments
                .is_empty());
        }
        let closed = builder.push(&[0.0; FRAME_SAMPLES], 0.9, FrameFlags::GAP_FILLED);
        assert_eq!(closed.segments[0].cut_reason, CutReason::Pause);
        assert!(closed.segments[0].samples[8 * FRAME_SAMPLES..]
            .iter()
            .all(|v| *v == 0.0));
    }

    #[test]
    fn final_flush_matches_reference_without_pre_roll() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        frames(&mut builder, 100, 0.0);
        frames(&mut builder, 8, 0.9);
        let closed = builder.finish();
        assert_eq!(closed.segments[0].start, StreamTime(100 * 512));
        assert_eq!(closed.segments[0].end, StreamTime(108 * 512));
        assert_eq!(closed.segments[0].samples.len(), 8 * FRAME_SAMPLES);
        assert!(builder.finish().segments.is_empty());
    }

    #[test]
    fn post_roll_longer_than_silence_waits_for_available_samples() {
        let config = VadConfig {
            min_silence_s: 0.16,
            post_roll_s: 0.5,
            ..VadConfig::default()
        };
        let mut builder = SegmentBuilder::new(config);
        frames(&mut builder, 8, 0.9);
        assert!(frames(&mut builder, 5, 0.0).segments.is_empty());
        let closed = frames(&mut builder, 11, 0.0);
        assert_eq!(closed.segments.len(), 1);
        assert_eq!(closed.segments[0].end, StreamTime(8 * 512 + 8_000));
        assert_eq!(
            closed.segments[0].samples.len() as u64,
            closed.segments[0].end.0
        );
    }

    #[test]
    fn history_stays_bounded_for_long_speech_and_idle_streams() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        for _ in 0..3_600 {
            builder.push(&[1.0; FRAME_SAMPLES], 0.9, FrameFlags::EMPTY);
            assert!(builder.history.len() <= 324);
        }
        builder.finish();
        for _ in 0..3_600 {
            builder.push(&[0.0; FRAME_SAMPLES], 0.0, FrameFlags::EMPTY);
            assert!(builder.history.len() <= 18);
        }
    }

    #[derive(Deserialize)]
    struct Fixture {
        params: FixtureParams,
        probs: Vec<f32>,
        energy: Vec<f32>,
        segments: Vec<ExpectedSegment>,
    }

    #[derive(Deserialize)]
    struct FixtureParams {
        thr: f32,
        min_speech: f32,
        min_silence: f32,
        pre: f32,
        post: f32,
        soft_after: f32,
        soft_thr: f32,
        hard_at: f32,
    }

    #[derive(Deserialize)]
    struct ExpectedSegment {
        start: f64,
        end: f64,
        reason: String,
    }

    fn assert_fixture_parity(json: &str, count: usize) {
        let fixture: Fixture = serde_json::from_str(json).unwrap();
        let p = fixture.params;
        let config = VadConfig {
            threshold: p.thr,
            min_speech_s: p.min_speech,
            min_silence_s: p.min_silence,
            pre_roll_s: p.pre,
            post_roll_s: p.post,
            soft_cut_after_s: p.soft_after,
            soft_cut_prob: p.soft_thr,
            hard_cut_s: p.hard_at,
            ..VadConfig::default()
        };
        let mut builder = SegmentBuilder::new(config);
        let mut output = SegmentUpdate::default();
        assert_eq!(fixture.probs.len(), fixture.energy.len());
        for (probability, energy) in fixture.probs.iter().zip(&fixture.energy) {
            append(
                &mut output,
                builder.push(
                    &[energy.sqrt(); FRAME_SAMPLES],
                    *probability,
                    FrameFlags::EMPTY,
                ),
            );
        }
        append(&mut output, builder.finish());
        assert_eq!(fixture.segments.len(), count);
        assert_eq!(output.segments.len(), count);
        for (index, (actual, expected)) in output.segments.iter().zip(&fixture.segments).enumerate()
        {
            let start = StreamTime::from_seconds(expected.start);
            let end = StreamTime::from_seconds(expected.end);
            assert!(
                actual.start.0.abs_diff(start.0) <= FRAME_SAMPLES as u64,
                "start {index}: {:?} vs {start:?}",
                actual.start
            );
            assert!(
                actual.end.0.abs_diff(end.0) <= FRAME_SAMPLES as u64,
                "end {index}: {:?} vs {end:?}",
                actual.end
            );
            let reason = match expected.reason.as_str() {
                "pause" => CutReason::Pause,
                "soft cut" => CutReason::SoftCut,
                "hard cut" => CutReason::HardCut,
                "end" => CutReason::End,
                other => panic!("unexpected fixture reason {other}"),
            };
            assert_eq!(actual.cut_reason, reason, "reason {index}");
            assert_eq!(actual.samples.len() as u64, actual.end.0 - actual.start.0);
            assert_eq!(actual.id, UtteranceId(index as u64));
        }
    }

    #[test]
    fn keynote_fixture_matches_all_58_segments() {
        assert_fixture_parity(include_str!("../../../testdata/segmenter-leijun.json"), 58);
    }

    #[test]
    fn conversation_fixture_matches_all_42_segments() {
        assert_fixture_parity(include_str!("../../../testdata/segmenter-ramc.json"), 42);
    }
}
