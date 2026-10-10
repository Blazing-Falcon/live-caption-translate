//! The X-ASR, Fun-ASR-Nano and Qwen3-ASR engines on the 20 ASCEND clips. Real inference
//! requires LT_MODELS_DIR with those models and the test audio.

use lt_core::engines::WindowAsr;
use lt_sherpa::{FunAsrNano, Qwen3Asr, XAsr};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Clip {
    file: String,
    #[serde(rename = "ref")]
    reference: String,
}

#[derive(Deserialize)]
struct Reference {
    xasr: Vec<String>,
}

fn wav(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 16_000);
    reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.unwrap()) / 32768.0)
        .collect()
}

/// Character error rate over letters and digits, ignoring case, spaces and punctuation.
fn errors(reference: &str, hypothesis: &str) -> (usize, usize) {
    let keep = |text: &str| -> Vec<char> {
        text.to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect()
    };
    let (reference, hypothesis) = (keep(reference), keep(hypothesis));
    let mut row: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, r) in reference.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, h) in hypothesis.iter().enumerate() {
            let next = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(r != h));
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    (row[hypothesis.len()], reference.len())
}

/// X-ASR reproduces the recorded outputs; Qwen3-ASR stays accurate; the output cap keeps
/// every clip of the two speech LLMs short, even when one starts to loop.
#[test]
fn second_pass_engines_match_references_and_stay_bounded() {
    let Some(models) = std::env::var_os("LT_MODELS_DIR").map(PathBuf::from) else {
        eprintln!("Real model test skipped: set LT_MODELS_DIR to verified model directory");
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let audio = std::env::var_os("LT_REFERENCE_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("testdata/fetched"))
        .join("ascend");
    let clips: Vec<Clip> =
        serde_json::from_slice(&std::fs::read(audio.join("clips.json")).unwrap()).unwrap();
    let reference: Reference =
        serde_json::from_slice(include_bytes!("fixtures/xasr-ascend.json")).unwrap();
    let samples: Vec<Vec<f32>> = clips
        .iter()
        .map(|clip| wav(&audio.join(&clip.file)))
        .collect();

    let mut xasr = XAsr::new(models.join(lt_sherpa::XASR_MODEL), 2).unwrap();
    let mut matched = 0;
    for (index, clip) in samples.iter().enumerate() {
        let text = xasr.decode_window(clip).unwrap().text;
        if text == reference.xasr[index] {
            matched += 1;
        } else {
            eprintln!(
                "xasr_mismatch clip={index} expected={:?} actual={text:?}",
                reference.xasr[index]
            );
        }
    }
    eprintln!("xasr_parity matched={matched}/{}", samples.len());
    assert!(
        matched >= 19,
        "X-ASR matched fewer than 19/20 recorded outputs"
    );
    drop(xasr);

    // Only one large model is loaded at a time.
    for (name, max_cer) in [("qwen3_asr", Some(0.20)), ("funasr_nano", None)] {
        let mut engine: Box<dyn WindowAsr> = if name == "qwen3_asr" {
            Box::new(Qwen3Asr::new(models.join(lt_sherpa::QWEN3_ASR_MODEL), 2).unwrap())
        } else {
            Box::new(FunAsrNano::new(models.join(lt_sherpa::FUNASR_NANO_MODEL), 2, true).unwrap())
        };
        let (mut wrong, mut total) = (0, 0);
        let mut slowest = Duration::ZERO;
        for (clip, samples) in clips.iter().zip(&samples) {
            let started = Instant::now();
            let text = engine.decode_window(samples).unwrap().text;
            slowest = slowest.max(started.elapsed());
            let (e, n) = errors(&clip.reference, &text);
            wrong += e;
            total += n;
        }
        let cer = wrong as f64 / total as f64;
        eprintln!(
            "second_pass engine={name} cer={cer:.3} slowest_ms={}",
            slowest.as_millis()
        );
        // The output cap bounds every clip to seconds, also when the model loops.
        assert!(
            slowest < Duration::from_secs(20),
            "{name}: a clip ran past the output cap"
        );
        if let Some(max_cer) = max_cer {
            assert!(cer <= max_cer, "{name}: character error rate {cer:.3}");
        }
    }
}
