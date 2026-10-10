//! Streaming X-ASR on the 20 ASCEND clips, fed in 100 ms pieces like live capture. Real
//! inference requires LT_MODELS_DIR with the streaming model and the test audio.

use lt_core::{config::AsrConfig, engines::StreamingAsr};
use lt_sherpa::XAsrStream;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Clip {
    file: String,
    #[serde(rename = "ref")]
    reference: String,
}

fn wav(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 16_000);
    reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.unwrap()) / 32768.0)
        .collect()
}

/// Character errors over letters and digits, ignoring case, spaces and punctuation.
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

/// The clause tracker cuts on tokens it has seen: a later result must only add tokens.
#[test]
fn streaming_tokens_only_grow_and_the_text_stays_accurate() {
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
    let settings = AsrConfig::default();
    let mut engine = XAsrStream::new(
        models.join(lt_sherpa::xasr_stream_model(settings.stream_chunk_ms)),
        &settings,
    )
    .unwrap();
    let (mut wrong, mut total) = (0, 0);
    for clip in &clips {
        let samples = wav(&audio.join(&clip.file));
        let mut seen: Vec<String> = Vec::new();
        for piece in samples.chunks(1_600) {
            engine.accept(piece).unwrap();
            let result = engine.result().unwrap();
            assert!(
                result.tokens.tokens.starts_with(&seen),
                "{}: emitted tokens changed",
                clip.file
            );
            assert_eq!(
                result.tokens.tokens.len(),
                result.tokens.timestamps.len(),
                "one start time per token"
            );
            seen = result.tokens.tokens;
        }
        let last = engine.finish().unwrap();
        assert!(last.tokens.tokens.starts_with(&seen));
        let (e, n) = errors(&clip.reference, &last.tokens.tokens.concat());
        wrong += e;
        total += n;
    }
    let cer = wrong as f64 / total as f64;
    eprintln!("xasr_stream cer={cer:.3}");
    assert!(cer <= 0.12, "character error rate {cer:.3}");
}
