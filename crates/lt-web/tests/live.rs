use lt_core::{
    config::TranslateEngine,
    engines::{TranslateRequest, TranslationControl, Translator},
    types::UtteranceId,
};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

fn run_with(engine: TranslateEngine, alternate: bool) {
    if std::env::var("LT_WEB_LIVE").as_deref() != Ok("1") {
        eprintln!("Live {engine} test skipped: set LT_WEB_LIVE=1");
        return;
    }
    let mut translator = if alternate {
        lt_web::WebTranslator::google_alt()
    } else {
        lt_web::WebTranslator::new(engine)
    }
    .unwrap();
    let control = TranslationControl {
        deadline: Instant::now() + Duration::from_secs(60),
        cancelled: Arc::new(AtomicBool::new(false)),
        abort: Arc::new(AtomicBool::new(false)),
    };
    translator.warm_up(&control).unwrap();
    let mut deltas = Vec::new();
    let answer = translator
        .translate(
            &TranslateRequest {
                id: UtteranceId(1),
                text: "今天的会议改到下午三点。",
                max_tokens: None,
                control: control.clone(),
            },
            &mut |text| deltas.push(text.to_owned()),
        )
        .unwrap();
    assert!(!answer.text.trim().is_empty());
    assert_eq!(deltas, [answer.text]);
    let sources: Vec<_> = (1..=30).map(|i| format!("第{i}次会议明天开始。")).collect();
    let refs: Vec<_> = sources.iter().map(String::as_str).collect();
    let answers = translator.translate_batch(&refs, &control).unwrap();
    assert_eq!(answers.len(), 30);
    assert!(answers.iter().all(|answer| !answer.text.is_empty()));
    println!("Live {engine}: sentence and 30-text batch passed");
}

#[test]
fn microsoft_sentence_and_batch() {
    run_with(TranslateEngine::Microsoft, false);
}

#[test]
fn google_sentence_and_batch() {
    run_with(TranslateEngine::Google, false);
}

#[test]
fn openai_compatible_sentence() {
    if std::env::var("LT_WEB_LIVE").as_deref() != Ok("1") {
        eprintln!("Live OpenAI-compatible test skipped: set LT_WEB_LIVE=1");
        return;
    }
    let (Ok(base_url), Ok(model)) = (
        std::env::var("LT_OPENAI_BASE_URL"),
        std::env::var("LT_OPENAI_MODEL"),
    ) else {
        eprintln!("Live OpenAI-compatible test skipped: LT_OPENAI_BASE_URL and LT_OPENAI_MODEL are required");
        return;
    };
    if base_url.trim().is_empty() || model.trim().is_empty() {
        eprintln!("Live OpenAI-compatible test skipped: endpoint and model must be nonempty");
        return;
    }
    let mut translator = lt_web::OpenAiTranslator::new(&lt_core::config::OpenAiConfig {
        base_url,
        model,
        api_key: std::env::var("LT_OPENAI_API_KEY").unwrap_or_default(),
        prompt: String::new(),
    })
    .unwrap();
    let answer = translator
        .translate(
            &TranslateRequest {
                id: UtteranceId(1),
                text: "今天的会议改到下午三点。",
                max_tokens: None,
                control: TranslationControl {
                    deadline: Instant::now() + Duration::from_secs(10),
                    cancelled: Arc::new(AtomicBool::new(false)),
                    abort: Arc::new(AtomicBool::new(false)),
                },
            },
            &mut |_| {},
        )
        .unwrap();
    assert!(!answer.text.trim().is_empty());
    println!("Live OpenAI-compatible sentence passed");
}

#[test]
fn google_alt_sentence_and_batch() {
    run_with(TranslateEngine::Google, true);
}
