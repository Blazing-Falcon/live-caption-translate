//! Slow gain control, initialized from the first audible block to avoid startup lag.

use lt_core::{config::AudioConfig, types::SAMPLE_RATE};

const TARGET_RMS: f64 = 0.1;

pub struct LevelNormalizer {
    config: AudioConfig,
    gain_db: Option<f64>,
}

impl LevelNormalizer {
    pub fn new(config: AudioConfig) -> Self {
        Self {
            config,
            gain_db: None,
        }
    }
    pub fn reset(&mut self) {
        self.gain_db = None;
    }
    pub fn gain_db(&self) -> f32 {
        self.gain_db.unwrap_or(0.0) as f32
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.config.normalize || samples.is_empty() {
            return;
        }
        let energy = samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / samples.len() as f64;
        let rms = energy.sqrt();
        let gate = 10_f64.powf(f64::from(self.config.norm_gate_dbfs) / 20.0);
        if !rms.is_finite() || rms <= gate {
            return;
        }
        let desired =
            (20.0 * (TARGET_RMS / rms).log10()).min(f64::from(self.config.norm_max_gain_db));
        let gain_db = self
            .gain_db
            .map_or(desired, |previous| {
                let time = if desired < previous {
                    self.config.norm_attack_s
                } else {
                    self.config.norm_release_s
                };
                let seconds = samples.len() as f64 / SAMPLE_RATE as f64;
                let fraction = 1.0 - (-seconds / f64::from(time).max(0.001)).exp();
                previous + fraction * (desired - previous)
            })
            .min(f64::from(self.config.norm_max_gain_db));
        self.gain_db = Some(gain_db);
        let gain = 10_f64.powf(gain_db / 20.0) as f32;
        for sample in samples {
            *sample *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::{
        engines::Vad,
        fakes::FakeVad,
        segment::{FrameFlags, SegmentBuilder},
        types::{CutReason, Segment},
    };

    fn segmented(amplitude: f32) -> Vec<Segment> {
        let mut normalizer = LevelNormalizer::new(AudioConfig::default());
        let mut vad = FakeVad::from_energy(0.005);
        let mut builder = SegmentBuilder::new(lt_core::config::VadConfig::default());
        let mut segments = Vec::new();
        for index in 0..120 {
            let mut frame = [0.0; 512];
            if (15..90).contains(&index) {
                for (sample_index, sample) in frame.iter_mut().enumerate() {
                    *sample = amplitude
                        * (std::f32::consts::TAU * 1_000.0 * sample_index as f32 / 16_000.0).sin();
                }
            }
            normalizer.process(&mut frame);
            assert!(normalizer.gain_db() <= 20.0);
            segments.extend(
                builder
                    .push(&frame, vad.speech_prob(&frame), FrameFlags::EMPTY)
                    .segments,
            );
        }
        segments.extend(builder.finish().segments);
        segments
    }

    #[test]
    fn quiet_and_full_level_speech_have_the_same_segment_boundaries() {
        let quiet = segmented(10_f32.powf(-30.0 / 20.0));
        let loud = segmented(1.0);
        assert_eq!(quiet.len(), 1);
        assert_eq!(loud.len(), 1);
        assert_eq!(quiet[0].cut_reason, CutReason::Pause);
        assert_eq!(quiet[0].start, loud[0].start);
        assert_eq!(quiet[0].end, loud[0].end);
    }
}
