//! `lt-cli screen` on a rendered article: the real recognizer and the real translator. Requires
//! LT_MODELS_DIR with the OCR and translation models and LT_LLAMA_SERVER.
#![cfg(feature = "ocr")]

use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
fn every_block_of_the_article_is_translated() {
    if std::env::var_os("LT_MODELS_DIR").is_none() || std::env::var_os("LT_LLAMA_SERVER").is_none()
    {
        eprintln!("Real model test skipped: set LT_MODELS_DIR and LT_LLAMA_SERVER");
        return;
    }
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::env::temp_dir().join(format!("lt-cli-screen-{}.json", std::process::id()));
    let run = Command::new(env!("CARGO_BIN_EXE_lt-cli"))
        .current_dir(&workspace)
        .args([
            "screen",
            "testdata/screen/article@1.png",
            "--explain",
            "--out",
        ])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let output: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(&out).unwrap();

    let events: Vec<&Value> = output["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["event"])
        .collect();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["reading", "blocks", "translated", "translated", "done"],
        "{events:#?}"
    );
    let blocks = events[1]["blocks"].as_array().unwrap();
    assert!(!output["decisions"].as_array().unwrap().is_empty());
    assert_eq!(
        output["grouping_trace"]["groups"].as_array().unwrap().len(),
        2
    );
    assert_eq!(blocks.len(), 2);
    assert!(blocks.iter().all(|block| block["translate"] == true));
    for event in &events[2..4] {
        let text = event["text"].as_str().unwrap();
        assert!(!text.is_empty());
        assert!(
            !text.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
            "Chinese left in {text:?}"
        );
    }
}
