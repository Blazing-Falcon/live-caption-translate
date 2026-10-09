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

/// Acceptance: the 40 sentences of `mt_hy_vs_lmt.json` through the draft server, first
/// drafts on the chat path, a prefilled retry on `/completion`, and a comparison of the chat
/// path with `/completion` and an empty prefill.
#[test]
fn draft_sentences_against_configured_llama_server() {
    use lt_core::config::LatencyConfig;
    use lt_llm::{supervisor::ServerRole, LmtDraftTranslator, LmtPrompts};
    let Some(binary) = std::env::var_os("LT_LLAMA_SERVER") else {
        eprintln!("Skipping real draft fixtures: LT_LLAMA_SERVER is unset");
        return;
    };
    let Some(model) = std::env::var_os("LT_DRAFT_MODEL").map(PathBuf::from) else {
        eprintln!("Skipping real draft fixtures: LT_DRAFT_MODEL is unset");
        return;
    };
    assert!(model.is_file(), "draft model missing: {}", model.display());
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let config = TranslateConfig {
        threads: 2,
        ..TranslateConfig::default()
    };
    let mut supervisor = Supervisor::start(
        ServerOptions::new(PathBuf::from(binary), model, &config, 8)
            .with_role(ServerRole::Draft)
            .with_below_normal(true),
        EventBus::default(),
    )
    .unwrap();
    supervisor.wait_ready(Duration::from_secs(70)).unwrap();
    let mut translator =
        LmtDraftTranslator::new(supervisor.url(), &LatencyConfig::default()).unwrap();
    let fixture: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/mt_hy_vs_lmt.json")).unwrap();
    assert_eq!(fixture.len(), 40);
    let control = || TranslationControl {
        deadline: Instant::now() + Duration::from_secs(10),
        cancelled: Arc::new(AtomicBool::new(false)),
        abort: Arc::new(AtomicBool::new(false)),
    };
    let request = |text: &'static str, prefill: &'static str| TranslateRequest {
        id: UtteranceId(1),
        text,
        src: "zh",
        tgt: "en",
        terms: &[],
        context: &[],
        prefill,
        max_tokens: Some(64),
        control: control(),
    };
    let http = ureq::Agent::new_with_defaults();
    let mut outputs = Vec::new();
    let mut identical_empty_prefill = 0;
    let mut prefix_kept = 0;
    let mut chat_ms = Vec::new();
    for sentence in &fixture {
        let source: &'static str =
            Box::leak(sentence["zh"].as_str().unwrap().to_owned().into_boxed_str());
        let started = Instant::now();
        let chat = translator
            .translate(&request(source, ""), &mut |_| {})
            .unwrap();
        chat_ms.push(started.elapsed().as_millis());
        assert!(!chat.text.is_empty(), "empty draft for {source}");
        // The same sentence on /completion with an empty prefill.
        let body = json!({
            "prompt": LmtPrompts::completion_prompt(source, ""),
            "n_predict": 64, "temperature": 0, "cache_prompt": true,
            "stop": ["<|im_end|>", "\n\n"], "stream": false,
        });
        let mut reply = http
            .post(&format!("{}/completion", supervisor.url()))
            .send_json(body)
            .unwrap();
        let raw: Value = reply.body_mut().read_json().unwrap();
        let completion = raw["content"].as_str().unwrap().trim().to_owned();
        let same = completion == chat.text;
        identical_empty_prefill += usize::from(same);
        // A prefilled request returns text that continues from the first words of the chat draft.
        let words: Vec<&str> = chat.text.split_whitespace().collect();
        let keep = words.len().saturating_sub(2).max(1).min(words.len());
        let prefill: &'static str =
            Box::leak(format!("{} ", words[..keep].join(" ")).into_boxed_str());
        let prefilled = translator
            .translate(&request(source, prefill), &mut |_| {})
            .unwrap();
        let rejoined = format!("{prefill}{}", prefilled.text);
        let starts = rejoined.starts_with(prefill);
        prefix_kept += usize::from(starts);
        outputs.push(json!({
            "zh": source, "chat": chat.text, "completion_empty_prefill": completion,
            "identical": same, "prefill": prefill, "continuation": prefilled.text, "rejoined": rejoined,
        }));
    }
    supervisor.stop().unwrap();
    let path = workspace.join("target/tmp/draft-fixture-outputs.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "total": fixture.len(), "chat_equals_completion_empty_prefill": identical_empty_prefill,
            "prefill_kept": prefix_kept, "chat_ms": chat_ms, "outputs": outputs,
        }))
        .unwrap(),
    )
    .unwrap();
    eprintln!(
        "Draft fixtures: chat == /completion(empty prefill) for {identical_empty_prefill}/40; prefill kept {prefix_kept}/40; outputs: {}",
        path.display()
    );
    assert_eq!(prefix_kept, 40);
}
