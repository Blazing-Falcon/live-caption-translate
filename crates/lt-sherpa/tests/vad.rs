//! Terminal-only Silero parity tests. Real inference requires LT_MODELS_DIR.

use lt_core::{
    config::VadConfig,
    engines::Vad,
    segment::{FrameFlags, SegmentBuilder},
};
use lt_sherpa::{onnxruntime_version, SileroVad};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Deserialize)]
struct VadReference {
    start_s: f64,
    dur_s: f64,
    probs: Vec<f32>,
    segments: Vec<ExpectedSegment>,
}

#[derive(Deserialize)]
struct ExpectedSegment {
    start: f64,
    end: f64,
}

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

#[test]
fn missing_silero_model_fails_with_readable_startup_error() {
    let missing =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/missing-lt-sherpa-model.onnx");
    assert!(!missing.exists());
    let error = match SileroVad::with_runtime(&missing, &missing) {
        Ok(_) => panic!("Missing Silero model accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Silero VAD v5 model is missing"));
}

#[test]
fn silero_state_reset_and_probabilities_match_frozen_references() {
    let Some(models) = std::env::var_os("LT_MODELS_DIR") else {
        eprintln!("Real model test skipped: set LT_MODELS_DIR to verified model directory");
        return;
    };
    let models = PathBuf::from(models);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let audio = std::env::var_os("LT_REFERENCE_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("testdata/fetched"));
    let vad_path = models.join("silero-vad-v5/silero_vad_v5.onnx");
    let mut vad = SileroVad::new(vad_path).unwrap();
    eprintln!("shared_onnxruntime_version={}", onnxruntime_version());

    // State reset must reproduce the initial model state exactly on this build.
    let initial = vad.try_speech_prob(&[0.0; 512]).unwrap();
    for _ in 0..8 {
        vad.try_speech_prob(&[0.1; 512]).unwrap();
    }
    vad.reset();
    assert_eq!(vad.try_speech_prob(&[0.0; 512]).unwrap(), initial);

    for (fixture, clip) in [
        ("segmenter-leijun.json", "leijun/lei-jun.wav"),
        ("segmenter-ramc.json", "ramc/CTS-CN-F2F-2019-11-15-1449.wav"),
    ] {
        let reference: VadReference =
            serde_json::from_slice(&std::fs::read(root.join("testdata").join(fixture)).unwrap())
                .unwrap();
        let samples = wav(&audio.join(clip));
        let start = (reference.start_s * 16_000.0).round() as usize;
        let length = (reference.dur_s * 16_000.0).round() as usize;
        let samples = &samples[start..start + length];
        let mut builder = SegmentBuilder::new(VadConfig::default());
        let mut segments = Vec::new();
        let mut maximum_error = 0.0_f32;
        let started = Instant::now();
        vad.reset();
        for (index, frame) in samples.as_chunks::<512>().0.iter().enumerate() {
            let probability = vad.try_speech_prob(frame).unwrap();
            maximum_error = maximum_error.max((probability - reference.probs[index]).abs());
            segments.extend(builder.push(frame, probability, FrameFlags::EMPTY).segments);
        }
        segments.extend(builder.finish().segments);
        assert_eq!(samples.len() / 512, reference.probs.len());
        let matched: usize =
            reference
                .segments
                .iter()
                .map(|expected| {
                    usize::from(segments.iter().any(|actual| {
                        (actual.start.seconds() - expected.start).abs() <= 0.064 + 1e-6
                    })) + usize::from(
                        segments.iter().any(|actual| {
                            (actual.end.seconds() - expected.end).abs() <= 0.064 + 1e-6
                        }),
                    )
                })
                .sum();
        let boundary_fraction = matched as f64 / (reference.segments.len() * 2) as f64;
        let count_error = segments.len().abs_diff(reference.segments.len()) as f64
            / reference.segments.len() as f64;
        eprintln!("vad_parity clip={clip} frames={} expected_segments={} actual_segments={} boundaries_within_2_frames={:.3} maximum_probability_error={maximum_error:.8} elapsed_ms={}", reference.probs.len(), reference.segments.len(), segments.len(), boundary_fraction, started.elapsed().as_millis());
        assert!(
            count_error <= 0.05,
            "{clip}: segment count differs by more than 5%"
        );
        assert!(
            boundary_fraction >= 0.90,
            "{clip}: fewer than 90% of boundaries match"
        );
        assert!(
            maximum_error <= 0.005,
            "{clip}: probabilities diverged from the reference"
        );
    }
}
