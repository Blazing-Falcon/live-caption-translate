//! Silero v5's real per-frame probability, with the Python reference's state.

use crate::runtime::{default_runtime_path, engine_error, initialize_shared_runtime, require_file};
use lt_core::{
    engines::Vad,
    error::{Error, Result},
};
use ort::{session::Session, value::TensorRef};
use std::path::Path;

const CONTEXT: usize = 64;
const FRAME: usize = 512;
const STATE: usize = 2 * 128;

pub struct SileroVad {
    session: Session,
    context: [f32; CONTEXT],
    state: [f32; STATE],
    input: [f32; CONTEXT + FRAME],
    last_error: Option<String>,
}

impl SileroVad {
    pub fn new(model: impl AsRef<Path>) -> Result<Self> {
        Self::with_runtime(model, default_runtime_path()?)
    }

    pub fn with_runtime(model: impl AsRef<Path>, runtime: impl AsRef<Path>) -> Result<Self> {
        let model = model.as_ref();
        require_file(model, "Silero VAD v5 model")?;
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
            .map_err(|error| engine_error("Cannot open the Silero VAD v5 model", error))?;
        for expected in ["input", "state", "sr"] {
            if !session
                .inputs()
                .iter()
                .any(|input| input.name() == expected)
            {
                return Err(Error::Engine(format!(
                    "Silero VAD model is missing its {expected} input"
                )));
            }
        }
        if session.outputs().len() != 2 {
            return Err(Error::Engine(
                "Silero VAD v5 must return probability and recurrent state".into(),
            ));
        }
        Ok(Self {
            session,
            context: [0.0; CONTEXT],
            state: [0.0; STATE],
            input: [0.0; CONTEXT + FRAME],
            last_error: None,
        })
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn try_speech_prob(&mut self, frame: &[f32; FRAME]) -> Result<f32> {
        if frame.iter().any(|sample| !sample.is_finite()) {
            return Err(Error::Engine(
                "Silero VAD received nonfinite audio samples".into(),
            ));
        }
        self.input[..CONTEXT].copy_from_slice(&self.context);
        self.input[CONTEXT..].copy_from_slice(frame);
        let sample_rate = [16_000_i64];
        let input = TensorRef::from_array_view(([1_usize, CONTEXT + FRAME], self.input.as_slice()))
            .map_err(|error| engine_error("Cannot prepare Silero waveform input", error))?;
        let state = TensorRef::from_array_view(([2_usize, 1, 128], self.state.as_slice()))
            .map_err(|error| engine_error("Cannot prepare Silero recurrent state", error))?;
        let rate = TensorRef::from_array_view(((), sample_rate.as_slice()))
            .map_err(|error| engine_error("Cannot prepare Silero sample rate", error))?;
        let outputs = self
            .session
            .run(ort::inputs! { "input" => input, "state" => state, "sr" => rate })
            .map_err(|error| engine_error("Silero VAD inference failed", error))?;
        if outputs.len() != 2 {
            return Err(Error::Engine(
                "Silero VAD returned incomplete outputs".into(),
            ));
        }
        let (_, probabilities) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine_error("Invalid Silero probability tensor", error))?;
        let (shape, next_state) = outputs[1]
            .try_extract_tensor::<f32>()
            .map_err(|error| engine_error("Invalid Silero recurrent-state tensor", error))?;
        let probability = probabilities
            .first()
            .copied()
            .ok_or_else(|| Error::Engine("Silero VAD returned no probability".into()))?;
        if probabilities.len() != 1
            || !probability.is_finite()
            || !(0.0..=1.0).contains(&probability)
        {
            return Err(Error::Engine(
                "Silero VAD returned an invalid speech probability".into(),
            ));
        }
        if shape.as_ref() != [2_i64, 1, 128]
            || next_state.len() != STATE
            || next_state.iter().any(|value| !value.is_finite())
        {
            return Err(Error::Engine(
                "Silero VAD returned an invalid recurrent state".into(),
            ));
        }
        self.state.copy_from_slice(next_state);
        self.context.copy_from_slice(&frame[FRAME - CONTEXT..]);
        self.last_error = None;
        Ok(probability)
    }
}

impl Vad for SileroVad {
    fn speech_prob(&mut self, frame: &[f32; FRAME]) -> f32 {
        match self.try_speech_prob(frame) {
            Ok(probability) => probability,
            Err(error) => {
                let message = error.to_string();
                if self.last_error.as_deref() != Some(&message) {
                    tracing::error!(message, "VAD inference failed");
                }
                self.last_error = Some(message);
                self.context.fill(0.0);
                self.state.fill(0.0);
                // SegmentBuilder treats nonfinite probability as silence; do
                // not falsely claim a valid 0.0 probability after an error.
                f32::NAN
            }
        }
    }

    fn reset(&mut self) {
        self.context.fill(0.0);
        self.state.fill(0.0);
        self.input.fill(0.0);
        self.last_error = None;
    }
}
