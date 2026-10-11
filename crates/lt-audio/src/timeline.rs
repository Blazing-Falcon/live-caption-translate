//! Continuous mono source-rate blocks, including missing packet intervals.
//! No allocations occur while accepting packets or synthesizing zeros.
use lt_core::{
    error::{Error, Result},
    source::FrameFlags,
};

pub const BLOCK_SAMPLES: usize = 512;

#[derive(Clone, Copy)]
pub struct MonoBlock {
    /// Source-rate frame position, before conversion to the 16 kHz stream.
    pub t0: u64,
    pub sample_rate: u32,
    pub samples: [f32; BLOCK_SAMPLES],
    pub frames: usize,
    pub flags: FrameFlags,
}

pub struct MonoTimeline {
    sample_rate: u32,
    cursor: u64,
    block: MonoBlock,
    all_gap: bool,
}

impl MonoTimeline {
    pub fn new(sample_rate: u32) -> Result<Self> {
        if sample_rate == 0 {
            return Err(Error::Engine("Source sample rate must be positive".into()));
        }
        Ok(Self {
            sample_rate,
            cursor: 0,
            block: MonoBlock {
                t0: 0,
                sample_rate,
                samples: [0.0; BLOCK_SAMPLES],
                frames: 0,
                flags: FrameFlags::EMPTY,
            },
            all_gap: true,
        })
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Stale/overlapping samples cannot replace audio already published. A
    /// future packet first fills its timestamp hole with synthesized silence.
    pub fn push_packet(
        &mut self,
        t0: u64,
        pcm: &[f32],
        silent: bool,
        emit: &mut impl FnMut(MonoBlock) -> Result<()>,
    ) -> Result<()> {
        if !silent && pcm.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Engine(
                "Audio packet contains nonfinite samples".into(),
            ));
        }
        if pcm.is_empty() {
            return Ok(());
        }
        self.timeout_until(t0, emit)?;
        let skip = self.cursor.saturating_sub(t0).min(pcm.len() as u64) as usize;
        if silent {
            self.zeros(pcm.len().saturating_sub(skip) as u64, emit)
        } else {
            for sample in &pcm[skip..] {
                self.sample(*sample, false, emit)?;
            }
            Ok(())
        }
    }

    /// Call only with a confirmed capture timeout watermark. Leave a small
    /// delivery margin when deriving it from wall/QPC time, so a late packet
    /// is not preemptively replaced with zeros.
    pub fn timeout_until(
        &mut self,
        raw_end: u64,
        emit: &mut impl FnMut(MonoBlock) -> Result<()>,
    ) -> Result<()> {
        self.zeros(raw_end.saturating_sub(self.cursor), emit)
    }

    fn zeros(
        &mut self,
        mut frames: u64,
        emit: &mut impl FnMut(MonoBlock) -> Result<()>,
    ) -> Result<()> {
        while frames != 0 {
            let count = frames.min((BLOCK_SAMPLES - self.block.frames) as u64) as usize;
            let end = self.block.frames + count;
            self.block.samples[self.block.frames..end].fill(0.0);
            self.block.frames = end;
            self.cursor = self
                .cursor
                .checked_add(count as u64)
                .ok_or_else(|| Error::Engine("Audio source clock overflow".into()))?;
            frames -= count as u64;
            if self.block.frames == BLOCK_SAMPLES {
                self.flush(emit)?;
            }
        }
        Ok(())
    }

    fn sample(
        &mut self,
        sample: f32,
        gap: bool,
        emit: &mut impl FnMut(MonoBlock) -> Result<()>,
    ) -> Result<()> {
        self.block.samples[self.block.frames] = sample;
        self.block.frames += 1;
        self.all_gap &= gap;
        self.cursor = self
            .cursor
            .checked_add(1)
            .ok_or_else(|| Error::Engine("Audio source clock overflow".into()))?;
        if self.block.frames == BLOCK_SAMPLES {
            self.flush(emit)?;
        }
        Ok(())
    }

    /// Preserve valid partial length; resamplers decide how to flush/filter it.
    pub fn flush(&mut self, emit: &mut impl FnMut(MonoBlock) -> Result<()>) -> Result<()> {
        if self.block.frames == 0 {
            return Ok(());
        }
        self.block.flags.gap_filled = self.all_gap;
        emit(self.block)?;
        self.block = MonoBlock {
            t0: self.cursor,
            sample_rate: self.sample_rate,
            samples: [0.0; BLOCK_SAMPLES],
            frames: 0,
            flags: FrameFlags::EMPTY,
        };
        self.all_gap = true;
        Ok(())
    }

    /// A real source replacement, distinct from an ordinary missing packet.
    pub fn discontinuity(
        &mut self,
        t0: u64,
        emit: &mut impl FnMut(MonoBlock) -> Result<()>,
    ) -> Result<()> {
        self.flush(emit)?;
        self.cursor = t0;
        self.block.t0 = t0;
        self.block.flags.discontinuity = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stalled device still advances the stream with silence, so the recognizer's pause rule
    /// can end the open clause.
    #[test]
    fn five_second_packet_hole_is_filled_with_silence_up_to_the_deadline() {
        let mut timeline = MonoTimeline::new(16_000).unwrap();
        let mut blocks = Vec::new();
        let mut emit = |block: MonoBlock| {
            assert_eq!(block.frames, 512);
            blocks.push(block);
            Ok(())
        };
        timeline
            .push_packet(0, &[0.1; 16_384], false, &mut emit)
            .unwrap();
        timeline
            .timeout_until(16_384 + 5 * 16_000, &mut emit)
            .unwrap();
        let filled: Vec<_> = blocks
            .iter()
            .filter(|block| block.flags.gap_filled)
            .collect();
        assert_eq!(filled.first().map(|block| block.t0), Some(16_384));
        assert!(filled
            .iter()
            .all(|block| block.samples.iter().all(|s| *s == 0.0)));
        let end = blocks.last().map(|block| block.t0 + block.frames as u64);
        assert!(end.is_some_and(|end| end + 512 > 16_384 + 5 * 16_000));
    }

    #[test]
    fn overlaps_holes_partial_silence_and_discontinuity_keep_one_clock() {
        let mut timeline = MonoTimeline::new(48_000).unwrap();
        let mut blocks = Vec::new();
        let mut emit = |block| {
            blocks.push(block);
            Ok(())
        };
        timeline
            .push_packet(0, &[0.5; 300], false, &mut emit)
            .unwrap();
        timeline
            .push_packet(200, &[0.75; 200], false, &mut emit)
            .unwrap();
        timeline
            .push_packet(512, &[f32::NAN; 512], true, &mut emit)
            .unwrap();
        timeline.discontinuity(2_000, &mut emit).unwrap();
        timeline
            .push_packet(2_000, &[0.25; 7], false, &mut emit)
            .unwrap();
        timeline.flush(&mut emit).unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].t0, 0);
        assert_eq!(&blocks[0].samples[300..400], &[0.75; 100]);
        assert!(blocks[0].samples[400..].iter().all(|sample| *sample == 0.0));
        assert!(!blocks[0].flags.gap_filled);
        assert_eq!(blocks[1].t0, 512);
        assert!(blocks[1].flags.gap_filled);
        assert!(!blocks[1].flags.discontinuity);
        assert_eq!(blocks[2].t0, 2_000);
        assert_eq!(blocks[2].frames, 7);
        assert!(blocks[2].flags.discontinuity);
        assert!(!blocks[2].flags.gap_filled);
        assert_eq!(timeline.cursor(), 2_007);
    }
}
