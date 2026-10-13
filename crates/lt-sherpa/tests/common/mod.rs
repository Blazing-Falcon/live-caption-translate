//! Shared by the real-model tests: the ASCEND clips and the character error count.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct Clip {
    pub file: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

pub fn wav(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, 16_000);
    reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.unwrap()) / 32768.0)
        .collect()
}

/// Character errors over letters and digits, ignoring case, spaces and punctuation.
pub fn errors(reference: &str, hypothesis: &str) -> (usize, usize) {
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

/// The ASCEND folder and its clips, or `None` when the models are not set.
pub fn ascend() -> Option<(PathBuf, PathBuf, Vec<Clip>)> {
    let Some(models) = std::env::var_os("LT_MODELS_DIR").map(PathBuf::from) else {
        eprintln!("Real model test skipped: set LT_MODELS_DIR to verified model directory");
        return None;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let audio = std::env::var_os("LT_REFERENCE_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("testdata/fetched"))
        .join("ascend");
    let clips = serde_json::from_slice(&std::fs::read(audio.join("clips.json")).unwrap()).unwrap();
    Some((models, audio, clips))
}
