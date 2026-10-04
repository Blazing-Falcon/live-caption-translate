//! Mono streaming conversion to 16 kHz with exact cumulative finite-clip counts.
//!
//! The 48 kHz branch uses a 64-tap Blackman-windowed sinc, cutoff 7.2 kHz,
//! normalized to unity DC. Three polyphase sums run only for retained samples.
//! Its group delay is 31.5 input samples; trimming ten output samples aligns the
//! retained grid to within half an input sample. Other rates use rubato's FFT
//! converter, trimming its reported delay once and flushing at EOF.

use lt_core::error::{Error, Result};
use rubato::{audioadapter_buffers::direct::InterleavedSlice, Fft, FixedSync, Indexing, Resampler};

const TAPS: usize = 64;
const RATE_OUT: u32 = 16_000;

pub struct Decimator48 {
    coefficients: [f64; TAPS],
    history: [f32; TAPS],
    cursor: usize,
    phase: usize,
}

impl Default for Decimator48 {
    fn default() -> Self {
        Self::new()
    }
}

impl Decimator48 {
    pub fn new() -> Self {
        let mut coefficients = [0.0; TAPS];
        for (index, coefficient) in coefficients.iter_mut().enumerate() {
            let offset = index as f64 - (TAPS - 1) as f64 / 2.0;
            let sinc =
                (std::f64::consts::TAU * 0.15 * offset).sin() / (std::f64::consts::PI * offset);
            let phase = std::f64::consts::TAU * index as f64 / (TAPS - 1) as f64;
            let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            *coefficient = sinc * window;
        }
        let sum = coefficients.iter().sum::<f64>();
        for coefficient in &mut coefficients {
            *coefficient /= sum;
        }
        Self {
            coefficients,
            history: [0.0; TAPS],
            cursor: 0,
            phase: 0,
        }
    }

    /// Appends floor(cumulative input / 3) causal samples across arbitrary blocks.
    pub fn push(&mut self, input: &[f32], output: &mut Vec<f32>) {
        output.reserve((input.len() + self.phase) / 3);
        for sample in input {
            self.history[self.cursor] = *sample;
            self.cursor = (self.cursor + 1) % TAPS;
            self.phase += 1;
            if self.phase != 3 {
                continue;
            }
            self.phase = 0;
            let mut value = 0.0;
            for phase in 0..3 {
                for tap in (phase..TAPS).step_by(3) {
                    let sample = self.history[(self.cursor + TAPS - 1 - tap) % TAPS];
                    value += self.coefficients[tap] * f64::from(sample);
                }
            }
            output.push(value as f32);
        }
    }
}

struct FftState {
    converter: Fft<f32>,
    input: Vec<f32>,
    output: Vec<f32>,
    filled: usize,
}

impl FftState {
    fn new(rate: u32) -> Result<Self> {
        let converter = Fft::<f32>::new(
            rate as usize,
            RATE_OUT as usize,
            (rate as usize / 100).max(1),
            1,
            FixedSync::Both,
        )
        .map_err(|error| Error::Engine(format!("Could not create resampler: {error}")))?;
        let input = vec![0.0; converter.input_frames_max()];
        let output = vec![0.0; converter.output_frames_max()];
        Ok(Self {
            converter,
            input,
            output,
            filled: 0,
        })
    }

    fn chunk(&mut self, partial: Option<usize>, raw: &mut Vec<f32>) -> Result<()> {
        let input_frames = self.converter.input_frames_next();
        let output_frames = self.converter.output_frames_next();
        if input_frames > self.input.len() || output_frames > self.output.len() {
            return Err(Error::Engine(
                "Resampler changed its fixed buffer requirements".into(),
            ));
        }
        let input = InterleavedSlice::new(&self.input, 1, self.input.len())
            .map_err(|error| Error::Engine(format!("Invalid resampler input: {error}")))?;
        let output_capacity = self.output.len();
        let mut output = InterleavedSlice::new_mut(&mut self.output, 1, output_capacity)
            .map_err(|error| Error::Engine(format!("Invalid resampler output: {error}")))?;
        let indexing = Indexing {
            partial_len: partial,
            ..Indexing::default()
        };
        let (_, produced) = self
            .converter
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|error| Error::Engine(format!("Resampling failed: {error}")))?;
        if produced == 0 {
            return Err(Error::Engine("Resampler produced no frames".into()));
        }
        raw.extend_from_slice(&self.output[..produced]);
        self.filled = 0;
        Ok(())
    }

    fn push(&mut self, mut input: &[f32], raw: &mut Vec<f32>) -> Result<()> {
        while !input.is_empty() {
            let wanted = self.converter.input_frames_next();
            let take = (wanted - self.filled).min(input.len());
            self.input[self.filled..self.filled + take].copy_from_slice(&input[..take]);
            self.filled += take;
            input = &input[take..];
            if self.filled == wanted {
                self.chunk(None, raw)?;
            }
        }
        Ok(())
    }
}

enum Conversion {
    Identity,
    Decimator(Box<Decimator48>),
    Fft(Box<FftState>),
}

pub struct MonoResampler {
    rate: u32,
    conversion: Conversion,
    input_samples: u64,
    emitted: u64,
    delay_left: usize,
    raw: Vec<f32>,
    finished: bool,
}

impl MonoResampler {
    pub fn new(rate: u32) -> Result<Self> {
        if rate == 0 {
            return Err(Error::Engine("Audio sample rate must be positive".into()));
        }
        let (conversion, delay_left) = match rate {
            RATE_OUT => (Conversion::Identity, 0),
            48_000 => (Conversion::Decimator(Box::default()), 10),
            _ => {
                let state = FftState::new(rate)?;
                let delay = state.converter.output_delay();
                (Conversion::Fft(Box::new(state)), delay)
            }
        };
        Ok(Self {
            rate,
            conversion,
            input_samples: 0,
            emitted: 0,
            delay_left,
            raw: Vec::new(),
            finished: false,
        })
    }

    pub fn input_rate(&self) -> u32 {
        self.rate
    }

    pub fn push(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<()> {
        if self.finished {
            return Err(Error::Engine(
                "Resampler already reached end of stream".into(),
            ));
        }
        self.input_samples = self.input_samples.saturating_add(input.len() as u64);
        self.raw.clear();
        match &mut self.conversion {
            Conversion::Identity => self.raw.extend_from_slice(input),
            Conversion::Decimator(converter) => converter.push(input, &mut self.raw),
            Conversion::Fft(converter) => converter.push(input, &mut self.raw)?,
        }
        self.accept(output);
        Ok(())
    }

    pub fn finish(&mut self, output: &mut Vec<f32>) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let target = self.target_len();
        while self.emitted < target {
            self.raw.clear();
            match &mut self.conversion {
                Conversion::Identity => break,
                Conversion::Decimator(converter) => converter.push(&[0.0; TAPS], &mut self.raw),
                Conversion::Fft(converter) => {
                    converter.chunk(Some(converter.filled), &mut self.raw)?
                }
            }
            self.accept(output);
        }
        Ok(())
    }

    fn target_len(&self) -> u64 {
        (u128::from(self.input_samples) * u128::from(RATE_OUT))
            .div_ceil(u128::from(self.rate))
            .min(u128::from(u64::MAX)) as u64
    }

    fn accept(&mut self, output: &mut Vec<f32>) {
        let skip = self.delay_left.min(self.raw.len());
        self.delay_left -= skip;
        let available = self.target_len().saturating_sub(self.emitted);
        let count = available.min((self.raw.len() - skip) as u64) as usize;
        output.extend_from_slice(&self.raw[skip..skip + count]);
        self.emitted += count as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubato::audioadapter::Adapter;

    fn tone(frequency: f64, rate: usize, count: usize) -> Vec<f32> {
        (0..count)
            .map(|sample| {
                (std::f64::consts::TAU * frequency * sample as f64 / rate as f64).sin() as f32
            })
            .collect()
    }
    fn rms(samples: &[f32]) -> f64 {
        (samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }
    fn convert(input: &[f32], rate: u32, partition: usize) -> Vec<f32> {
        let mut converter = MonoResampler::new(rate).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(partition) {
            converter.push(chunk, &mut out).unwrap();
        }
        converter.finish(&mut out).unwrap();
        assert!(out.iter().all(|sample| sample.is_finite()));
        out
    }

    #[test]
    fn decimator_passes_1khz_and_rejects_9khz_across_uneven_blocks() {
        for (frequency, threshold) in [(1_000.0_f64, -0.5_f64), (9_000.0, -40.0)] {
            let input = tone(frequency, 48_000, 48_000);
            let mut converter = Decimator48::new();
            let mut out = Vec::new();
            for chunk in input.chunks(137) {
                converter.push(chunk, &mut out);
            }
            assert_eq!(out.len(), 16_000);
            let db = 20.0 * (rms(&out[100..]) / rms(&input[300..])).log10();
            if frequency == 1_000.0 {
                assert!(db.abs() <= threshold.abs(), "passband {db} dB");
            } else {
                assert!(db <= threshold, "stopband {db} dB");
            }
            let mut single = Vec::new();
            Decimator48::new().push(&input, &mut single);
            assert_eq!(single, out);
        }
    }

    #[test]
    fn exact_counts_include_short_clips_empty_input_and_flush_tail() {
        for rate in [8_000, 16_000, 22_050, 44_100, 48_000] {
            for count in [0, 1, 7, 100, rate as usize] {
                let input = tone(1_000.0, rate as usize, count);
                let expected = (count as u128 * 16_000).div_ceil(u128::from(rate)) as usize;
                let contiguous = convert(&input, rate, count.max(1));
                let partitioned = convert(&input, rate, 17);
                assert_eq!(contiguous.len(), expected, "rate={rate} count={count}");
                assert_eq!(contiguous, partitioned, "rate={rate} count={count}");
            }
        }
        assert_eq!(convert(&vec![0.0; 441_000], 44_100, 997).len(), 160_000);
    }

    #[test]
    fn fft_streaming_matches_whole_clip_with_delay_trimmed_once() {
        let input = tone(900.0, 44_100, 1_003);
        let actual = convert(&input, 44_100, 73);
        let adapter = InterleavedSlice::new(&input, 1, input.len()).unwrap();
        let mut fft = Fft::<f32>::new(44_100, 16_000, 441, 1, FixedSync::Both).unwrap();
        let expected = fft.process_all(&adapter, input.len(), None).unwrap();
        assert_eq!(actual.len(), expected.frames());
        for (index, sample) in actual.iter().enumerate() {
            assert!((*sample - expected.read_sample(0, index).unwrap()).abs() < 0.000_001);
        }
        let mut impulses = vec![0.0; 2_048];
        impulses[0] = 1.0;
        impulses[2_047] = 1.0;
        let output = convert(&impulses, 44_100, 103);
        assert!(output[..8].iter().any(|sample| sample.abs() > 0.01));
        assert!(output[output.len() - 8..]
            .iter()
            .any(|sample| sample.abs() > 0.01));
    }
}
