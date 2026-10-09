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
    /// Where the current uninterrupted run of speech began. Commit cuts move `start` but not
    /// this, so soft and hard cuts keep firing where they would without commits.
    run_start: u64,
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
                        run_start: start,
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
                let duration = self.current.saturating_sub(speech.run_start);
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
                    speech.run_start = self.current;
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
                    speech.run_start = cut;
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

    /// The open clause and where its `Segment` would start (pre-roll included).
    pub fn open_clause(&self) -> Option<(UtteranceId, StreamTime)> {
        self.speech
            .as_ref()
            .map(|speech| (speech.id, StreamTime(self.start_sample(speech))))
    }

    /// Audio of the open clause from stream sample `from` (or its start, if later) to now.
    pub fn open_audio(&self, from: u64) -> Option<(UtteranceId, Vec<f32>)> {
        let speech = self.speech.as_ref()?;
        let start = self.start_sample(speech).max(from);
        let end = self.current;
        let mut pcm = Vec::with_capacity(end.saturating_sub(start) as usize);
        for frame in &self.history {
            let frame_end = frame.start.saturating_add(FRAME_SAMPLES as u64);
            if frame_end <= start || frame.start >= end {
                continue;
            }
            let first = start.saturating_sub(frame.start) as usize;
            let last = (end.min(frame_end) - frame.start) as usize;
            pcm.extend_from_slice(&frame.samples[first..last]);
        }
        Some((speech.id, pcm))
    }

    /// Cut the open clause `id` at `at` because the recognizer committed a clause there.
    /// Returns an empty update if `id` is not the open clause or `at` is not strictly inside it.
    /// Soft and hard cut clocks are not reset (they follow `run_start`).
    pub fn commit_cut(&mut self, id: UtteranceId, at: StreamTime) -> SegmentUpdate {
        let mut update = SegmentUpdate::default();
        let valid = self.speech.as_ref().is_some_and(|speech| {
            speech.id == id && self.start_sample(speech) < at.0 && at.0 < self.current
        });
        if !valid {
            return update;
        }
        if let Some(mut speech) = self.speech.take() {
            self.emit(&speech, at.0, CutReason::Commit, &mut update);
            speech.id = self.allocate_id();
            speech.start = at.0;
            speech.pre_roll = false;
            update.started.push((speech.id, at));
            self.speech = Some(speech);
        }
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

    fn frame_with(index: usize) -> [f32; FRAME_SAMPLES] {
        [(index as f32 + 1.0) * 0.001; FRAME_SAMPLES]
    }

    fn feed(
        builder: &mut SegmentBuilder,
        output: &mut SegmentUpdate,
        range: std::ops::Range<usize>,
        probability: f32,
    ) {
        for index in range {
            append(
                output,
                builder.push(&frame_with(index), probability, FrameFlags::EMPTY),
            );
        }
    }

    #[test]
    fn commit_cut_emits_the_left_part_with_exact_samples_and_opens_a_new_clause() {
        let mut builder = SegmentBuilder::new(VadConfig::default());
        let mut output = SegmentUpdate::default();
        feed(&mut builder, &mut output, 0..60, 0.9);
        assert_eq!(output.started, vec![(UtteranceId(0), StreamTime(0))]);
        let at = StreamTime(30 * 512 + 100);
        let cut = builder.commit_cut(UtteranceId(0), at);
        assert_eq!(cut.segments.len(), 1);
        let left = &cut.segments[0];
        assert_eq!(left.id, UtteranceId(0));
        assert_eq!(left.start, StreamTime(0));
        assert_eq!(left.end, at);
        assert_eq!(left.cut_reason, CutReason::Commit);
        assert_eq!(left.samples.len() as u64, at.0);
        for (position, sample) in left.samples.iter().enumerate() {
            assert_eq!(*sample, frame_with(position / FRAME_SAMPLES)[0]);
        }
        assert_eq!(cut.started, vec![(UtteranceId(1), at)]);
        feed(&mut builder, &mut output, 60..80, 0.9);
        let tail = builder.finish();
        assert_eq!(tail.segments[0].id, UtteranceId(1));
        assert_eq!(tail.segments[0].start, at);
        assert_eq!(tail.segments[0].end, StreamTime(80 * 512));
        assert_eq!(tail.segments[0].samples.len() as u64, 80 * 512 - at.0);
        assert_eq!(tail.segments[0].samples[0], frame_with(30)[0]);
    }

    #[test]
    fn forced_cuts_fire_at_the_same_positions_with_and_without_commit_cuts() {
        fn run(commits: bool) -> Vec<(CutReason, u64)> {
            let mut builder = SegmentBuilder::new(VadConfig::default());
            let mut output = SegmentUpdate::default();
            let mut open = UtteranceId(0);
            for index in 0..1_500 {
                // Dips below the soft threshold every 400 frames (12.8 s) to exercise both cuts.
                let probability = if index % 400 == 399 { 0.3 } else { 0.9 };
                let update = builder.push(&frame_with(index % 50), probability, FrameFlags::EMPTY);
                if let Some((id, _)) = update.started.last() {
                    open = *id;
                }
                append(&mut output, update);
                if commits && index % 90 == 89 {
                    let at = StreamTime(builder.current_time().0 - 20 * 512);
                    let update = builder.commit_cut(open, at);
                    if let Some((id, _)) = update.started.last() {
                        open = *id;
                    }
                    append(&mut output, update);
                }
            }
            append(&mut output, builder.finish());
            output
                .segments
                .iter()
                .filter(|s| matches!(s.cut_reason, CutReason::SoftCut | CutReason::HardCut))
                .map(|s| (s.cut_reason, s.end.0))
                .collect()
        }
        let plain = run(false);
        let with_commits = run(true);
        assert!(plain.len() >= 2, "{plain:?}");
        assert_eq!(plain.len(), with_commits.len());
        for (a, b) in plain.iter().zip(&with_commits) {
            assert_eq!(a.0, b.0, "cut kind");
            // Hard cuts search for the quietest frame inside the clause, so only the kind and
            // the stream second must agree.
            assert!(a.1.abs_diff(b.1) <= 32 * 512, "{a:?} vs {b:?}");
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
