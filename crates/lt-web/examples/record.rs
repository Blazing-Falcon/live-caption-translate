use lt_core::{config::TranslateEngine, engines::TranslationControl};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("LT_WEB_LIVE").as_deref() != Ok("1") {
        return Err("Set LT_WEB_LIVE=1 to record live answers".into());
    }
    let name = std::env::args().nth(1).ok_or("Expected engine name")?;
    let alternate = name == "google_alt";
    let engine: TranslateEngine = if alternate {
        TranslateEngine::Google
    } else {
        name.parse()?
    };
    let translator = if alternate {
        lt_web::WebTranslator::google_alt()?
    } else {
        lt_web::WebTranslator::new(engine)?
    };
    let control = TranslationControl {
        deadline: Instant::now() + Duration::from_secs(30),
        cancelled: Arc::new(AtomicBool::new(false)),
        abort: Arc::new(AtomicBool::new(false)),
    };
    let (raw, signature) = translator.record_response(
        &[
            "今天的会议改到下午三点。",
            "請在這裡等候。",
            "我们明天去公园。",
        ],
        &control,
    )?;
    let answers = if alternate {
        lt_web::parse_google_alt(&raw, 3)?
    } else {
        lt_web::parse_response(engine, &raw)?
    };
    if answers.len() != 3 || answers.iter().any(|text| text.trim().is_empty()) {
        return Err("Incomplete recording".into());
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(&name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("public.json"), raw)?;
    std::fs::write(
        dir.join("public.expected.json"),
        serde_json::to_string_pretty(&answers)?,
    )?;
    if !signature.is_null() && !alternate {
        std::fs::write(
            dir.join("signature.json"),
            serde_json::to_string_pretty(&signature)?,
        )?;
    }
    println!("Recorded {engine}: {}", serde_json::to_string(&answers)?);
    Ok(())
}
