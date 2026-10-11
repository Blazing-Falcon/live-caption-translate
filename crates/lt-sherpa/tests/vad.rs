//! TEN VAD parity with the reference library. Real inference requires LT_MODELS_DIR.

use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

fn wav(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    assert_eq!(
        spec.channels,
        1,
        "Reference WAV must be mono: {}",
        path.display()
    );
    assert_eq!(
        spec.sample_rate,
        16_000,
        "Reference WAV must be 16 kHz: {}",
        path.display()
    );
    match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|sample| sample.unwrap())
            .collect(),
        hound::SampleFormat::Int => {
            assert_eq!(spec.bits_per_sample, 16);
            reader
                .samples::<i16>()
                .map(|sample| f32::from(sample.unwrap()) / 32768.0)
                .collect()
        }
    }
}

#[derive(Deserialize)]
struct TenReference {
    wav: String,
    start_s: f64,
    dur_s: f64,
    probabilities: Vec<f32>,
}

/// The reference library also estimates pitch, which this port leaves at 0 Hz; the port must
/// still make nearly the same speech decisions, also across the 30 s state reset.
#[test]
fn ten_vad_decisions_follow_the_reference_library() {
    let Some(models) = std::env::var_os("LT_MODELS_DIR") else {
        eprintln!("Real model test skipped: set LT_MODELS_DIR to verified model directory");
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let audio = std::env::var_os("LT_REFERENCE_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("testdata/fetched"));
    let reference: TenReference =
        serde_json::from_slice(include_bytes!("fixtures/ten-vad-ramc.json")).unwrap();
    let samples = wav(&audio.join("ramc").join(&reference.wav));
    let start = (reference.start_s * 16_000.0).round() as usize;
    let length = (reference.dur_s * 16_000.0).round() as usize;
    let mut vad =
        lt_sherpa::TenVad::new(PathBuf::from(models).join("ten-vad/ten-vad.onnx")).unwrap();
    let started = Instant::now();
    let mut agree = 0;
    let mut total_error = 0.0_f64;
    for (hop, expected) in samples[start..start + length]
        .as_chunks::<256>()
        .0
        .iter()
        .zip(&reference.probabilities)
    {
        let probability = vad.try_hop_prob(hop).unwrap();
        agree += usize::from((probability > 0.5) == (*expected > 0.5));
        total_error += f64::from((probability - expected).abs());
    }
    let hops = reference.probabilities.len();
    let agreement = agree as f64 / hops as f64;
    eprintln!(
        "ten_vad_parity hops={hops} decisions_agree={agreement:.4} mean_abs_error={:.4} elapsed_ms={}",
        total_error / hops as f64,
        started.elapsed().as_millis()
    );
    assert!(
        agreement >= 0.97 && total_error / hops as f64 <= 0.04,
        "TEN VAD diverged from the reference library"
    );
}
