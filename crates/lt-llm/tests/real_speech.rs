//! Qwen3-ASR in llama-server reads each of the 20 ASCEND clips whole, as the second pass reads a
//! line's window. A broken request or answer format would not fail anything else: the second
//! pass would quietly keep the live text. Needs LT_LLAMA_SERVER, LT_MODELS_DIR with the model
//! and the test audio.

use lt_core::{config::AsrConfig, engines::OfflineAsr};
use lt_llm::speech::{Qwen3Server, QWEN3_MODEL};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

#[derive(Deserialize)]
struct Clip {
    file: String,
    #[serde(rename = "ref")]
    reference: String,
}

/// The samples of a 16 kHz mono 16-bit WAV file.
fn wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let mut at = 12;
    while &bytes[at..at + 4] != b"data" {
        at += 8 + u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
    }
    let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
    bytes[at + 8..at + 8 + len]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32_768.0)
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

#[test]
fn qwen3_in_llama_server_reads_whole_clips_accurately_with_and_without_a_forced_language() {
    let (Some(binary), Some(models)) = (
        std::env::var_os("LT_LLAMA_SERVER"),
        std::env::var_os("LT_MODELS_DIR").map(PathBuf::from),
    ) else {
        eprintln!("Real model test skipped: set LT_LLAMA_SERVER and LT_MODELS_DIR");
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let audio = std::env::var_os("LT_REFERENCE_AUDIO")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("testdata/fetched"))
        .join("ascend");
    let clips: Vec<Clip> =
        serde_json::from_slice(&std::fs::read(audio.join("clips.json")).unwrap()).unwrap();
    let settings = AsrConfig {
        second_pass_threads: 2,
        ..AsrConfig::default()
    };
    let mut engine = Qwen3Server::start(
        Path::new(&binary),
        &models.join(QWEN3_MODEL),
        &settings,
        false,
        &AtomicBool::new(false),
    )
    .unwrap();
    let (mut wrong, mut total) = (0, 0);
    let (mut forced_wrong, mut forced_total) = (0, 0);
    for clip in &clips {
        let samples = wav(&audio.join(&clip.file));
        let heard = engine.decode(&samples, None).unwrap();
        assert!(heard.lang.is_some(), "{}: no language", clip.file);
        let (e, n) = errors(&clip.reference, &heard.text);
        wrong += e;
        total += n;
        let forced = engine.decode(&samples, Some("zh")).unwrap();
        assert_eq!(forced.lang.as_deref(), Some("zh"));
        let (e, n) = errors(&clip.reference, &forced.text);
        forced_wrong += e;
        forced_total += n;
    }
    let cer = wrong as f64 / total as f64;
    let forced_cer = forced_wrong as f64 / forced_total as f64;
    eprintln!("qwen3 gguf cer={cer:.3} forced_zh_cer={forced_cer:.3}");
    assert!(cer <= 0.16, "character error rate {cer:.3}");
    assert!(
        forced_cer <= 0.16,
        "character error rate with zh {forced_cer:.3}"
    );
}
