//! TEN VAD through ort, with the feature frontend of the reference C library: 0.97
//! pre-emphasis, a 768-sample periodic Hann window every 256 samples, a 1024-point FFT,
//! 40 mel bands, log, the reference mean and deviation, and three frames of context.
//!
//! The reference library also gives the model a pitch estimate (a port of LPCNet's pitch
//! search). This port gives the model 0 Hz, as sherpa-onnx does, so its probabilities differ
//! from the reference library's in voiced frames.
//!
//! License: the constants and the frontend come from ten-vad (src/aed.cc, src/coeff.h), so
//! this file is a derivative work under the ten-vad Open Source License (Apache-2.0 with
//! Agora's additional conditions), not under the workspace license. Texts: notices/TEN-VAD-*.

use crate::runtime::{default_runtime_path, engine_error, initialize_shared_runtime, require_file};
use lt_core::{
    engines::Vad,
    error::{Error, Result},
};
use ort::{session::Session, value::TensorRef};
use std::{f32::consts::PI, path::Path};

const HOP: usize = 256;
const WINDOW: usize = 768;
const FFT: usize = 1024;
const BINS: usize = FFT / 2 + 1;
const BANDS: usize = 40;
const FEATURES: usize = BANDS + 1;
const CONTEXT: usize = 3;
const HIDDEN: usize = 64;
const STATES: usize = 4;
/// The reference clears the recurrent state every 30 s of audio.
const RESET_HOPS: usize = 1_875;
const PRE_EMPHASIS: f32 = 0.97;
const EPS: f32 = 1e-20;
/// Pitch in Hz given to the model; see the module comment.
const PITCH_HZ: f32 = 0.0;

#[rustfmt::skip]
const FEATURE_MEANS: [f32; FEATURES] = [
    -8.198236, -6.2657166, -5.4838185, -4.7586913, -4.417089,
    -4.142893, -3.9128504, -3.845928, -3.6570904, -3.7234187,
    -3.8761342, -3.843891, -3.6904051, -3.7560658, -3.6986961,
    -3.650463, -3.7004688, -3.5673213, -3.4989002, -3.477807,
    -3.458816, -3.4449239, -3.4013286, -3.3062613, -3.2785568,
    -3.2332509, -3.198616, -3.2045264, -3.2087986, -3.257838,
    -3.3813767, -3.5340214, -3.640868, -3.7268589, -3.773731,
    -3.8046672, -3.832901, -3.8711205, -3.990593, -4.4802895,
    92.3569,
];

#[rustfmt::skip]
const FEATURE_STDS: [f32; FEATURES] = [
    5.166064, 4.9772096, 4.698896, 4.6306214, 4.634348,
    4.641156, 4.6406765, 4.666367, 4.6505346, 4.640021,
    4.6374, 4.620099, 4.5963163, 4.562655, 4.5543604,
    4.5669107, 4.56249, 4.5624127, 4.5852995, 4.6001797,
    4.592846, 4.5859227, 4.5834966, 4.626093, 4.626958,
    4.6262894, 4.637006, 4.683016, 4.726814, 4.7342896,
    4.753227, 4.849723, 4.869435, 4.884483, 4.921327,
    4.9592123, 4.996619, 5.0448236, 5.072217, 5.0964394,
    115.21369,
];

pub struct TenVad {
    session: Session,
    inputs: [String; 1 + STATES],
    outputs: [String; 1 + STATES],
    frontend: Frontend,
    features: [f32; CONTEXT * FEATURES],
    states: [[f32; HIDDEN]; STATES],
    hops: usize,
    last_error: Option<String>,
}

impl TenVad {
    pub fn new(model: impl AsRef<Path>) -> Result<Self> {
        Self::with_runtime(model, default_runtime_path()?)
    }

    pub fn with_runtime(model: impl AsRef<Path>, runtime: impl AsRef<Path>) -> Result<Self> {
        let model = model.as_ref();
        require_file(model, "TEN VAD model")?;
        initialize_shared_runtime(runtime)?;
        let session = Session::builder()
            .and_then(|builder| builder.with_intra_threads(1).map_err(Into::into))
            .and_then(|builder| builder.with_inter_threads(1).map_err(Into::into))
            .and_then(|builder| builder.with_parallel_execution(false).map_err(Into::into))
            .and_then(|builder| {
                builder
                    .with_config_entry("session.intra_op.allow_spinning", "0")
                    .map_err(Into::into)
            })
            .and_then(|builder| {
                builder
                    .with_config_entry("session.inter_op.allow_spinning", "0")
                    .map_err(Into::into)
            })
            .and_then(|mut builder| builder.commit_from_file(model))
            .map_err(|error| engine_error("Cannot open the TEN VAD model", error))?;
        // The reference pairs the features with the first input and each state input with
        // the output at the same position.
        let names = |items: Vec<String>| -> Result<[String; 1 + STATES]> {
            items
                .try_into()
                .map_err(|_| Error::Engine("TEN VAD must have five inputs and five outputs".into()))
        };
        let inputs = names(
            session
                .inputs()
                .iter()
                .map(|i| i.name().to_owned())
                .collect(),
        )?;
        let outputs = names(
            session
                .outputs()
                .iter()
                .map(|o| o.name().to_owned())
                .collect(),
        )?;
        Ok(Self {
            session,
            inputs,
            outputs,
            frontend: Frontend::new(),
            features: [0.0; CONTEXT * FEATURES],
            states: [[0.0; HIDDEN]; STATES],
            hops: 0,
            last_error: None,
        })
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// The speech probability of one 256-sample hop (16 kHz, full scale ±1.0).
    pub fn try_hop_prob(&mut self, hop: &[f32; HOP]) -> Result<f32> {
        if hop.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Engine(
                "TEN VAD received nonfinite audio samples".into(),
            ));
        }
        let frame = self.frontend.features(hop);
        self.features.copy_within(FEATURES.., 0);
        self.features[(CONTEXT - 1) * FEATURES..].copy_from_slice(&frame);
        let features =
            TensorRef::from_array_view(([1_usize, CONTEXT, FEATURES], &self.features[..]))
                .map_err(|error| engine_error("Cannot prepare TEN VAD features", error))?;
        let mut inputs = vec![(self.inputs[0].clone(), features.into_dyn())];
        for (name, state) in self.inputs[1..].iter().zip(&self.states) {
            let tensor = TensorRef::from_array_view(([1_usize, HIDDEN], &state[..]))
                .map_err(|error| engine_error("Cannot prepare TEN VAD state", error))?;
            inputs.push((name.clone(), tensor.into_dyn()));
        }
        let outputs = self
            .session
            .run(inputs)
            .map_err(|error| engine_error("TEN VAD inference failed", error))?;
        let (_, probability) = outputs[self.outputs[0].as_str()]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine_error("Invalid TEN VAD probability tensor", error))?;
        let probability = probability
            .first()
            .copied()
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
            .ok_or_else(|| {
                Error::Engine("TEN VAD returned an invalid speech probability".into())
            })?;
        let mut next = [[0.0; HIDDEN]; STATES];
        for (name, state) in self.outputs[1..].iter().zip(&mut next) {
            let (_, values) = outputs[name.as_str()]
                .try_extract_tensor::<f32>()
                .map_err(|error| engine_error("Invalid TEN VAD state tensor", error))?;
            if values.len() != HIDDEN || values.iter().any(|v| !v.is_finite()) {
                return Err(Error::Engine("TEN VAD returned an invalid state".into()));
            }
            state.copy_from_slice(values);
        }
        drop(outputs);
        self.hops += 1;
        self.states = if self.hops.is_multiple_of(RESET_HOPS) {
            [[0.0; HIDDEN]; STATES]
        } else {
            next
        };
        self.last_error = None;
        Ok(probability)
    }

    fn clear(&mut self) {
        self.frontend = Frontend::new();
        self.features = [0.0; CONTEXT * FEATURES];
        self.states = [[0.0; HIDDEN]; STATES];
        self.hops = 0;
    }
}

impl Vad for TenVad {
    /// A 512-sample frame is two hops. The frame counts as speech when either hop does.
    fn speech_prob(&mut self, frame: &[f32; 512]) -> f32 {
        let (first, second) = frame.split_at(HOP);
        let result = [first, second]
            .into_iter()
            .map(|hop| self.try_hop_prob(hop.try_into().expect("half of a 512-sample frame")))
            .try_fold(0.0_f32, |max, probability| probability.map(|p| max.max(p)));
        match result {
            Ok(probability) => probability,
            Err(error) => {
                let message = error.to_string();
                if self.last_error.as_deref() != Some(&message) {
                    tracing::error!(message, "VAD inference failed");
                }
                self.clear();
                self.last_error = Some(message);
                // SegmentBuilder treats nonfinite probability as silence.
                f32::NAN
            }
        }
    }

    fn reset(&mut self) {
        self.clear();
        self.last_error = None;
    }
}

/// Pre-emphasis, STFT and log-mel features in the reference's int16 scale.
struct Frontend {
    previous: f32,
    analysis: [f32; WINDOW],
    window: [f32; WINDOW],
    mel: Vec<(usize, Vec<f32>)>,
    fft: Fft,
}

impl Frontend {
    fn new() -> Self {
        let mut window = [0.0; WINDOW];
        for (n, value) in window.iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (2.0 * PI * n as f32 / WINDOW as f32).cos();
        }
        Self {
            previous: 0.0,
            analysis: [0.0; WINDOW],
            window,
            mel: mel_bank(),
            fft: Fft::new(),
        }
    }

    fn features(&mut self, hop: &[f32; HOP]) -> [f32; FEATURES] {
        self.analysis.copy_within(HOP.., 0);
        for (target, &sample) in self.analysis[WINDOW - HOP..].iter_mut().zip(hop) {
            let sample = sample * 32_768.0;
            *target = sample - PRE_EMPHASIS * self.previous;
            self.previous = sample;
        }
        let mut real = [0.0_f32; FFT];
        for ((target, &sample), &weight) in real.iter_mut().zip(&self.analysis).zip(&self.window) {
            *target = sample * weight;
        }
        let power = self.fft.power(&real);
        let mut features = [0.0; FEATURES];
        for (band, (start, weights)) in self.mel.iter().enumerate() {
            let energy: f32 = weights
                .iter()
                .zip(&power[*start..])
                .map(|(weight, power)| weight * power)
                .sum();
            let value = (energy / (32_768.0 * 32_768.0) + EPS).ln();
            features[band] = (value - FEATURE_MEANS[band]) / (FEATURE_STDS[band] + EPS);
        }
        features[BANDS] = (PITCH_HZ - FEATURE_MEANS[BANDS]) / (FEATURE_STDS[BANDS] + EPS);
        features
    }
}

/// The reference's triangular HTK-mel bank over FFT bins, as (first bin, weights) per band.
fn mel_bank() -> Vec<(usize, Vec<f32>)> {
    let mel = |hz: f32| 2595.0 * (1.0 + hz / 700.0).log10();
    let (low, high) = (mel(0.0), mel(8_000.0));
    let edges: Vec<usize> = (0..BANDS + 2)
        .map(|i| {
            let point = i as f32 * (high - low) / (BANDS as f32 + 1.0) + low;
            let hz = 700.0 * (10_f32.powf(point / 2595.0) - 1.0);
            ((FFT as f32 + 1.0) * hz / 16_000.0) as usize
        })
        .collect();
    (0..BANDS)
        .map(|band| {
            let (left, center, right) = (edges[band], edges[band + 1], edges[band + 2]);
            let rising = (left..center).map(|i| (i - left) as f32 / (center - left) as f32);
            let falling = (center..right).map(|i| (right - i) as f32 / (right - center) as f32);
            (left, rising.chain(falling).collect())
        })
        .collect()
}

/// Radix-2 complex FFT of a real 1024-point frame; returns |X(k)|² for k = 0..=512.
struct Fft {
    twiddles: Vec<(f32, f32)>,
    reversed: Vec<usize>,
}

impl Fft {
    fn new() -> Self {
        let bits = FFT.trailing_zeros();
        Self {
            twiddles: (0..FFT / 2)
                .map(|k| {
                    let angle = -2.0 * std::f64::consts::PI * k as f64 / FFT as f64;
                    (angle.cos() as f32, angle.sin() as f32)
                })
                .collect(),
            reversed: (0..FFT)
                .map(|i| i.reverse_bits() >> (usize::BITS - bits))
                .collect(),
        }
    }

    fn power(&self, input: &[f32; FFT]) -> [f32; BINS] {
        let mut re = [0.0_f32; FFT];
        let mut im = [0.0_f32; FFT];
        for (i, &j) in self.reversed.iter().enumerate() {
            re[i] = input[j];
        }
        let mut size = 2;
        while size <= FFT {
            let half = size / 2;
            let step = FFT / size;
            for start in (0..FFT).step_by(size) {
                for k in 0..half {
                    let (wr, wi) = self.twiddles[k * step];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
        let mut power = [0.0; BINS];
        for (k, value) in power.iter_mut().enumerate() {
            *value = re[k] * re[k] + im[k] * im[k];
        }
        power
    }
}
