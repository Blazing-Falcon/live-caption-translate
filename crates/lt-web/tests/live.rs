use lt_core::{
    config::TranslateEngine,
    engines::{TranslateRequest, TranslationControl, Translator},
    types::UtteranceId,
};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

fn run(engine: TranslateEngine) {
    if std::env::var("LT_WEB_LIVE").as_deref() != Ok("1") {
        eprintln!("Live {engine} test skipped: set LT_WEB_LIVE=1");
        return;
    }
    let mut translator = lt_web::WebTranslator::new(engine).unwrap();
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
    run(TranslateEngine::Microsoft);
}

#[test]
fn google_sentence_and_batch() {
    run(TranslateEngine::Google);
}
