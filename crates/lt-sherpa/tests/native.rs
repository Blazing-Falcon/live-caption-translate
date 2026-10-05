//! Terminal-only joint-runtime and model parity tests. Set LT_MODELS_DIR to
//! enable real inference; model/audio downloads are handled outside this test.

use lt_core::{
    config::AsrConfig,
    engines::SegmentAsr,
    types::{CutReason, Segment, StreamTime, UtteranceId},
};
use lt_sherpa::{onnxruntime_version, verify_sensevoice_runtime, SenseVoiceAsr, SileroVad};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

#[derive(Deserialize)]
struct Clip {
    file: String,
    sv2024_auto: String,
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
fn sensevoice_runtime_preflight_negotiates_without_loading_a_model() {
    match verify_sensevoice_runtime() {
        Ok(()) => eprintln!(
            "sensevoice_api_28_supported runtime={}",
            onnxruntime_version()
        ),
        Err(error) => {
            assert!(error.to_string().contains("requires ONNX Runtime API 28"));
            eprintln!("sensevoice_api_28_rejected: {error}");
        }
    }
}

#[test]
fn missing_sensevoice_model_fails_with_readable_startup_error() {
    let missing =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/missing-lt-sherpa-model.onnx");
    assert!(!missing.exists());
    let error =
        match SenseVoiceAsr::with_runtime(&missing, &missing, &AsrConfig::default(), &missing) {
            Ok(_) => panic!("Missing SenseVoice model accepted"),
            Err(error) => error,
        };
    assert!(error
        .to_string()
        .contains("SenseVoice 2024 model is missing"));
}

#[test]
fn shared_runtime_preserves_native_metadata_and_matches_asr_references() {
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

    // Both engines coexist in this process and load the same shared runtime.
    let mut asr = SenseVoiceAsr::new(
        models.join("sensevoice-2024-07-17-int8/model.int8.onnx"),
        models.join("sensevoice-2024-07-17-int8/tokens.txt"),
        &AsrConfig::default(),
    )
    .unwrap();
    assert!(vad.try_speech_prob(&[0.0; 512]).unwrap().is_finite());
    let directory = audio.join("ascend");
    let clips: Vec<Clip> =
        serde_json::from_slice(&std::fs::read(directory.join("clips.json")).unwrap()).unwrap();
    assert_eq!(clips.len(), 20);
    let mut matched = 0;
    let started = Instant::now();
    for (index, clip) in clips.iter().enumerate() {
        let samples = wav(&directory.join(&clip.file));
        let segment = Segment {
            id: UtteranceId(index as u64),
            start: StreamTime::ZERO,
            end: StreamTime(samples.len() as u64),
            samples: Arc::from(samples),
            cut_reason: CutReason::End,
        };
        let transcript = asr.transcribe(&segment).unwrap();
        eprintln!(
            "asr_clip file={} asr_ms={} matches_reference={}",
            clip.file,
            transcript.timing.asr_ms,
            transcript.text == clip.sv2024_auto
        );
        assert!(
            transcript
                .lang_tag
                .as_deref()
                .is_some_and(|tag| tag.starts_with("<|")),
            "Missing native language metadata in {}",
            clip.file
        );
        assert!(
            transcript
                .event
                .as_deref()
                .is_some_and(|tag| tag.starts_with("<|")),
            "Missing native event metadata in {}",
            clip.file
        );
        if index == 0 {
            eprintln!("sensevoice_native_json={}", asr.last_result_json().unwrap());
        }
        if transcript.text == clip.sv2024_auto {
            matched += 1;
        } else {
            eprintln!(
                "asr_mismatch file={} expected={:?} actual={:?}",
                clip.file, clip.sv2024_auto, transcript.text
            );
        }
    }
    eprintln!(
        "asr_parity matched={matched}/20 elapsed_ms={}",
        started.elapsed().as_millis()
    );
    assert!(
        matched >= 19,
        "SenseVoice matched fewer than 19/20 frozen reference outputs"
    );
}
