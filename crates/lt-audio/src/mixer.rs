//! Bounded per-source frames mixed on one 16 kHz sample clock.
use lt_core::{
    error::{Error, Result},
    segment::FrameFlags,
    source::AudioFrame,
    types::StreamTime,
};
use std::collections::{BTreeMap, VecDeque};

const CAPACITY: usize = 64;
const MAX_SOURCES: usize = 32;

// Returning the rejected frame avoids allocating during backpressure.
#[allow(clippy::large_enum_variant)]
pub enum MixerPush {
    Accepted,
    Late,
    Full(AudioFrame),
}

pub struct ClockMixer {
    sources: BTreeMap<u64, VecDeque<AudioFrame>>,
    cursor: StreamTime,
    discontinuity: bool,
}

impl ClockMixer {
    pub fn new(at: StreamTime) -> Self {
        Self {
            sources: BTreeMap::new(),
            cursor: at,
            discontinuity: false,
        }
    }

    pub fn add_source(&mut self, id: u64) -> Result<()> {
        if self.sources.contains_key(&id) {
            return Ok(());
        }
        if self.sources.len() == MAX_SOURCES {
            return Err(Error::Engine(
                "At most 32 simultaneous audio sources can be mixed".into(),
            ));
        }
        self.sources.insert(id, VecDeque::with_capacity(CAPACITY));
        Ok(())
    }

    pub fn remove_source(&mut self, id: u64) {
        if self.sources.remove(&id).is_some() {
            self.discontinuity = true;
        }
    }

    pub fn reset_source(&mut self, id: u64) {
        if let Some(frames) = self.sources.get_mut(&id) {
            frames.clear();
        }
        self.discontinuity = true;
    }

    pub fn push(&mut self, id: u64, frame: AudioFrame) -> Result<MixerPush> {
        if frame.samples.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Engine("Mixer received nonfinite audio".into()));
        }
        let frames = self
            .sources
            .get_mut(&id)
            .ok_or_else(|| Error::Engine("Audio source is not registered with the mixer".into()))?;
        if frame.t0.samples().saturating_add(512) <= self.cursor.samples() {
            return Ok(MixerPush::Late);
        }
        if frames.back().is_some_and(|last| frame.t0 <= last.t0) {
            return Ok(MixerPush::Late);
        }
        if frames.len() == CAPACITY {
            return Ok(MixerPush::Full(frame));
        }
        frames.push_back(frame);
        Ok(MixerPush::Accepted)
    }

    /// A frame is emitted only once its complete interval is available. Missing
    /// streams contribute zeros; unaligned input frames retain sample offsets.
    pub fn take_until(&mut self, available: StreamTime) -> Option<AudioFrame> {
        let start = self.cursor.samples();
        let end = start.checked_add(512)?;
        if end > available.samples() {
            return None;
        }
        let mut samples = [0.0_f32; 512];
        let mut gap_only = true;
        let mut discontinuity = std::mem::take(&mut self.discontinuity);
        for frames in self.sources.values_mut() {
            while frames
                .front()
                .is_some_and(|frame| frame.t0.samples().saturating_add(512) <= start)
            {
                frames.pop_front();
            }
            for frame in frames.iter() {
                let input_start = frame.t0.samples();
                if input_start >= end {
                    break;
                }
                let input_end = input_start.saturating_add(512);
                let from = input_start.max(start);
                let to = input_end.min(end);
                if from >= to {
                    continue;
                }
                let output_offset = (from - start) as usize;
                let input_offset = (from - input_start) as usize;
                let count = (to - from) as usize;
                for index in 0..count {
                    samples[output_offset + index] += frame.samples[input_offset + index];
                }
                gap_only &= frame.flags.gap_filled;
                // Apply a source fence once, at its own first sample.
                if input_start >= start {
                    discontinuity |= frame.flags.discontinuity;
                }
            }
            while frames
                .front()
                .is_some_and(|frame| frame.t0.samples().saturating_add(512) <= end)
            {
                frames.pop_front();
            }
        }
        for sample in &mut samples {
            *sample = soft_limit(*sample);
        }
        self.cursor = StreamTime(end);
        Some(AudioFrame {
            t0: StreamTime(start),
            samples,
            flags: FrameFlags {
                gap_filled: gap_only,
                discontinuity,
            },
        })
    }

    pub fn current_time(&self) -> StreamTime {
        self.cursor
    }
}

fn soft_limit(sample: f32) -> f32 {
    let magnitude = sample.abs();
    if magnitude <= 0.8 {
        sample
    } else {
        sample.signum() * (0.8 + 0.2 * ((magnitude - 0.8) / 0.2).tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(at: u64, sample: f32) -> AudioFrame {
        AudioFrame {
            t0: StreamTime(at),
            samples: [sample; 512],
            flags: FrameFlags::EMPTY,
        }
    }

    #[test]
    fn source_offsets_gaps_removal_and_restart_preserve_one_continuous_clock() {
        let mut mixer = ClockMixer::new(StreamTime::ZERO);
        mixer.add_source(1).unwrap();
        mixer.add_source(2).unwrap();
        assert!(matches!(
            mixer.push(1, frame(0, 0.2)).unwrap(),
            MixerPush::Accepted
        ));
        mixer.push(2, frame(256, 0.3)).unwrap();
        assert!(mixer.take_until(StreamTime(511)).is_none());
        let mixed = mixer.take_until(StreamTime(512)).unwrap();
        assert_eq!(mixed.samples[..256], [0.2; 256]);
        assert_eq!(mixed.samples[256..], [0.5; 256]);
        let tail = mixer.take_until(StreamTime(1024)).unwrap();
        assert_eq!(tail.samples[..256], [0.3; 256]);
        assert_eq!(tail.samples[256..], [0.0; 256]);
        mixer.remove_source(2);
        let gap = mixer.take_until(StreamTime(1536)).unwrap();
        assert!(gap.flags.gap_filled && gap.flags.discontinuity);
        assert!(matches!(
            mixer.push(1, frame(0, 0.2)).unwrap(),
            MixerPush::Late
        ));
        mixer.reset_source(1);
        mixer.push(1, frame(1536, 0.4)).unwrap();
        let resumed = mixer.take_until(StreamTime(2048)).unwrap();
        assert!(resumed.flags.discontinuity);
        assert_eq!(resumed.samples, [0.4; 512]);
    }

    #[test]
    fn peak_limiter_is_smooth_bounded_and_quiet_mix_is_unmodified() {
        let mut mixer = ClockMixer::new(StreamTime::ZERO);
        for id in 0..4 {
            mixer.add_source(id).unwrap();
            mixer.push(id, frame(0, 0.9)).unwrap();
        }
        let mixed = mixer.take_until(StreamTime(512)).unwrap();
        assert!(mixed
            .samples
            .iter()
            .all(|sample| *sample <= 1.0 && *sample >= 0.8));
        assert_eq!(soft_limit(0.3), 0.3);
        assert_eq!(soft_limit(-0.3), -0.3);
        assert!((soft_limit(0.800_001) - 0.800_001).abs() < 0.000_001);
        assert_eq!(soft_limit(-2.0), -soft_limit(2.0));
    }

    #[test]
    fn backpressure_returns_the_frame_and_fences_are_applied_once() {
        let mut mixer = ClockMixer::new(StreamTime::ZERO);
        mixer.add_source(1).unwrap();
        for index in 0..CAPACITY {
            mixer.push(1, frame(index as u64 * 512 + 128, 0.1)).unwrap();
        }
        let full = match mixer
            .push(1, frame(CAPACITY as u64 * 512 + 128, 0.1))
            .unwrap()
        {
            MixerPush::Full(frame) => frame,
            _ => panic!("unbounded mixer"),
        };
        mixer.take_until(StreamTime(1024));
        mixer.take_until(StreamTime(1024));
        assert!(matches!(mixer.push(1, full).unwrap(), MixerPush::Accepted));
        mixer.reset_source(1);
        let mut fenced = frame(1024 + 128, 0.2);
        fenced.flags.discontinuity = true;
        mixer.push(1, fenced).unwrap();
        assert!(
            mixer
                .take_until(StreamTime(1536))
                .unwrap()
                .flags
                .discontinuity
        );
        assert!(
            !mixer
                .take_until(StreamTime(2048))
                .unwrap()
                .flags
                .discontinuity
        );
    }
}
