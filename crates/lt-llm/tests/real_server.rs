//! Optional native acceptance: enabled only when LT_LLAMA_SERVER is configured.
use lt_core::{
    bus::EventBus,
    config::TranslateConfig,
    engines::{TranslateRequest, TranslationControl, Translator},
    types::UtteranceId,
};
use lt_llm::{
    supervisor::{ServerOptions, Supervisor},
    OpenAiCompatTranslator,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

#[test]
fn frozen_translation_sentences_against_configured_llama_server() {
    let Some(binary) = std::env::var_os("LT_LLAMA_SERVER") else {
        eprintln!("Skipping real translation fixtures: LT_LLAMA_SERVER is unset");
        return;
    };
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let model = std::env::var_os("LT_MT_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let directory = std::env::var_os("LT_MODELS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| workspace.join("models"));
            directory.join("hy-mt2-1.8b-q4_0/Hy-MT2-1.8B.i1-Q4_0.gguf")
        });
    assert!(
        model.is_file(),
        "translation model missing: {}",
        model.display()
    );
    // Three MT threads leave one core for the app/ASR within the user's budget.
    let config = TranslateConfig {
        threads: 3,
        ..TranslateConfig::default()
    };
    let mut supervisor = Supervisor::start(
        ServerOptions::new(PathBuf::from(binary), model, &config, 4),
        EventBus::default(),
    )
    .unwrap();
    supervisor.wait_ready(Duration::from_secs(70)).unwrap();
    let mut translator = OpenAiCompatTranslator::new(supervisor.url(), config).unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../reference/fixtures/mt-sentences.json"
    ))
    .unwrap();
    let sentences = fixture["sentences"].as_array().unwrap();
    assert_eq!(sentences.len(), 37);
    let mut outputs = Vec::new();
    let mut exact_matches = 0;
    for (index, sentence) in sentences.iter().enumerate() {
        let source = sentence["src"].as_str().unwrap();
        let expected = sentence["en"].as_str().unwrap();
        let request = TranslateRequest {
            id: UtteranceId(index as u64 + 1),
            text: source,
            src: "zh",
            tgt: "en",
            terms: &[],
            context: &[],
            prefill: "",
            max_tokens: None,
            control: TranslationControl {
                deadline: Instant::now() + Duration::from_secs(10),
                cancelled: Arc::new(AtomicBool::new(false)),
                abort: Arc::new(AtomicBool::new(false)),
            },
        };
        let started = Instant::now();
        let mut first_token_ms = None;
        let output = translator
            .translate(&request, &mut |_| {
                if first_token_ms.is_none() {
                    first_token_ms = Some(started.elapsed().as_millis());
                }
            })
            .unwrap();
        assert!(
            !output.text.trim().is_empty(),
            "empty translation for fixture {index}"
        );
        let matches = output.text.trim() == expected;
        exact_matches += usize::from(matches);
        outputs.push(json!({
            "kind":sentence["kind"], "source":source, "expected":expected, "actual":output.text, "exact_match":matches,
            "wall_ms":started.elapsed().as_millis(), "first_token_ms":first_token_ms,
            "prompt_tokens":output.prompt_tokens, "cached_tokens":output.cached_tokens, "generated_tokens":output.generated_tokens,
        }));
    }
    supervisor.stop().unwrap();
    let path = workspace.join("target/tmp/mt-current-outputs.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "threads":3,"exact_matches":exact_matches,"total":sentences.len(),"outputs":outputs,
            "note":"Outputs from a different llama.cpp build require review of differing lines.",
        }))
        .unwrap(),
    )
    .unwrap();
    eprintln!(
        "Translation fixture exact matches: {exact_matches}/37; outputs: {}",
        path.display()
    );
}
