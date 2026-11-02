use lt_core::{
    config::TranslateEngine,
    engines::{TranslateRequest, TranslationControl, Translator},
    types::UtteranceId,
};
use serde_json::json;
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

const SENTENCES: [&str; 20] = [
    "今天的会议改到下午三点。",
    "请把窗户打开一点。",
    "明天早上我们一起去公园。",
    "这本书介绍了中国的传统节日。",
    "火车将在十分钟后到站。",
    "我想学习做一道新的菜。",
    "下雨的时候请记得带伞。",
    "图书馆周末也对公众开放。",
    "这个问题需要大家一起讨论。",
    "新的展览将在下周开始。",
    "请在这里写下你的建议。",
    "我们已经完成了第一部分。",
    "沿着这条路走就能看到车站。",
    "春天的花园里有很多花。",
    "今天下午的课程非常有意思。",
    "如果有问题请随时告诉我。",
    "这家商店每天九点开门。",
    "我把需要的材料放在桌上了。",
    "大家可以先休息十分钟。",
    "希望明年的旅行一切顺利。",
];

fn control(seconds: u64) -> TranslationControl {
    TranslationControl {
        deadline: Instant::now() + Duration::from_secs(seconds),
        cancelled: Arc::new(AtomicBool::new(false)),
        abort: Arc::new(AtomicBool::new(false)),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("LT_WEB_LIVE").as_deref() != Ok("1") {
        return Err("Set LT_WEB_LIVE=1 to measure fixed public test sentences".into());
    }
    let mut rows = Vec::new();
    for name in [
        "microsoft",
        "google",
        "google_alt",
        "google_beta",
        "bing",
        "youdao",
    ] {
        let mut translator = if name == "google_alt" {
            lt_web::WebTranslator::google_alt()?
        } else {
            lt_web::WebTranslator::new(name.parse::<TranslateEngine>()?)?
        };
        let mut latencies = Vec::new();
        let mut failures = Vec::new();
        for (index, text) in SENTENCES.iter().enumerate() {
            let began = Instant::now();
            let result = translator.translate(
                &TranslateRequest {
                    id: UtteranceId(index as u64),
                    text,
                    max_tokens: None,
                    control: control(15),
                },
                &mut |_| {},
            );
            latencies.push(began.elapsed().as_secs_f64() * 1000.0);
            if let Err(error) = result {
                failures.push(json!({"sentence": index, "error": error.to_string()}));
            }
        }
        let texts: Vec<_> = (1..=30).map(|i| format!("第{i}次会议明天开始。")).collect();
        let refs: Vec<_> = texts.iter().map(String::as_str).collect();
        let began = Instant::now();
        let batch = translator.translate_batch(&refs, &control(90));
        let batch_ms = began.elapsed().as_secs_f64() * 1000.0;
        let mut sorted = latencies.clone();
        sorted.sort_by(f64::total_cmp);
        let row = json!({"engine":name,"caption_ms":latencies,"caption_median_ms":(sorted[9]+sorted[10])/2.0,"caption_worst_ms":sorted[19],"caption_failures":failures,"batch_ms":batch_ms,"batch_count":batch.as_ref().map(|answers| answers.len()).ok(),"batch_error":batch.err().map(|error|error.to_string())});
        eprintln!(
            "{name}: {} captions passed, batch {}",
            20 - failures.len(),
            if row["batch_count"] == 30 {
                "passed"
            } else {
                "failed"
            }
        );
        rows.push(row);
    }
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}
