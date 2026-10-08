use crate::args::{BenchArgs, BenchEngine};
#[cfg(any(feature = "llm", feature = "sherpa"))]
use crate::engines;
#[cfg(not(all(feature = "sherpa", feature = "llm")))]
use anyhow::bail;
use anyhow::Result;
use lt_core::config::Config;
use std::sync::{atomic::AtomicBool, Arc};

pub fn bench(args: BenchArgs, config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
    match args.engine {
        BenchEngine::Asr => asr(args, config, cancelled),
        BenchEngine::Vad => vad(args, config, cancelled),
        BenchEngine::Mt => mt(args, config, cancelled),
    }
}

#[cfg(feature = "sherpa")]
fn asr(args: BenchArgs, config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
    use lt_core::{
        engines::SegmentAsr,
        types::{CutReason, Segment, StreamTime, UtteranceId},
    };
    use std::time::Instant;
    engines::check_cancel(&cancelled)?;
    let models = engines::models_dir(args.models.as_deref(), &config);
    let mut asr = lt_sherpa::SenseVoiceAsr::new(
        models.join("sensevoice-2024-07-17-int8/model.int8.onnx"),
        models.join("sensevoice-2024-07-17-int8/tokens.txt"),
        &config.asr,
    )?;
    for (id, path) in engines::wav_files(&args.input)?.into_iter().enumerate() {
        engines::check_cancel(&cancelled)?;
        let samples = lt_audio::wav::read_wav_mono16(&path)?;
        let end = StreamTime(samples.len() as u64);
        let segment = Segment {
            id: UtteranceId(id as u64),
            start: StreamTime::ZERO,
            end,
            samples: Arc::from(samples),
            cut_reason: CutReason::End,
        };
        let started = Instant::now();
        let output = asr.transcribe(&segment)?;
        println!(
            "{}",
            serde_json::json!({"file":path,"text":output.text,"lang":output.lang_tag,"event":output.event,"asr_ms":started.elapsed().as_millis(),"audio_s":end.seconds()})
        );
    }
    Ok(())
}
#[cfg(not(feature = "sherpa"))]
fn asr(_: BenchArgs, _: Config, _: Arc<AtomicBool>) -> Result<()> {
    bail!("ASR bench requires --features sherpa");
}

#[cfg(feature = "sherpa")]
fn vad(args: BenchArgs, config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
    use lt_core::{
        engines::Vad,
        segment::{FrameFlags, SegmentBuilder},
    };
    use std::time::Instant;
    engines::check_cancel(&cancelled)?;
    let models = engines::models_dir(args.models.as_deref(), &config);
    let mut vad = lt_sherpa::SileroVad::new(models.join("silero-vad-v5/silero_vad_v5.onnx"))?;
    for path in engines::wav_files(&args.input)? {
        engines::check_cancel(&cancelled)?;
        let samples = lt_audio::wav::read_wav_mono16(&path)?;
        let mut builder = SegmentBuilder::new(config.vad.clone());
        vad.reset();
        let started = Instant::now();
        let mut segments = Vec::new();
        for chunk in samples.chunks(512) {
            engines::check_cancel(&cancelled)?;
            let mut frame = [0.0; 512];
            frame[..chunk.len()].copy_from_slice(chunk);
            let probability = vad.try_speech_prob(&frame)?;
            segments.extend(
                builder
                    .push(&frame, probability, FrameFlags::EMPTY)
                    .segments,
            );
        }
        segments.extend(builder.finish().segments);
        let cuts: Vec<_> = segments.iter().map(|segment| serde_json::json!({"start":segment.start.seconds(),"end":segment.end.seconds(),"cut":segment.cut_reason})).collect();
        println!(
            "{}",
            serde_json::json!({"file":path,"vad_ms":started.elapsed().as_millis(),"audio_s":samples.len() as f64 / 16_000.0,"segments":cuts})
        );
    }
    Ok(())
}
#[cfg(not(feature = "sherpa"))]
fn vad(_: BenchArgs, _: Config, _: Arc<AtomicBool>) -> Result<()> {
    bail!("VAD bench requires --features sherpa");
}

#[cfg(feature = "llm")]
fn mt(args: BenchArgs, config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
    use lt_core::{
        bus::EventBus,
        engines::{TranslateRequest, TranslationControl},
        types::UtteranceId,
    };
    use std::time::{Duration, Instant};
    let models = engines::models_dir(args.models.as_deref(), &config);
    let bus = EventBus::default();
    let (mut translator, supervisor, _) =
        engines::translator(&config, &models, args.server.as_deref(), &bus, &cancelled)?;
    let control = || TranslationControl {
        deadline: Instant::now() + Duration::from_secs_f32(config.translate.timeout_s),
        cancelled: cancelled.clone(),
        abort: Arc::new(AtomicBool::new(false)),
    };
    translator.warm_up(&control())?;
    let fixture = if args.input.is_dir() {
        std::path::PathBuf::from("reference/fixtures/mt-sentences.json")
    } else {
        args.input
    };
    let sentences: serde_json::Value = serde_json::from_slice(&std::fs::read(fixture)?)?;
    let sentences = sentences
        .get("sentences")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("MT fixture must contain a sentences array"))?;
    let mut matches = 0;
    let mut processed = 0;
    for (id, sentence) in sentences.iter().enumerate() {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        let text = sentence
            .get("src")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Sentence missing src"))?;
        let started = Instant::now();
        let mut first = None;
        let output = match translator.translate(
            &TranslateRequest {
                id: UtteranceId(id as u64),
                text,
                src: "zh",
                tgt: "en",
                terms: &[],
                context: &[],
                prefill: "",
                max_tokens: None,
                control: control(),
            },
            &mut |text| {
                if !text.is_empty() {
                    first.get_or_insert_with(|| started.elapsed().as_millis());
                }
            },
        ) {
            Ok(output) => output,
            Err(lt_core::error::Error::Stopped)
                if cancelled.load(std::sync::atomic::Ordering::Acquire) =>
            {
                break
            }
            Err(error) => return Err(error.into()),
        };
        processed += 1;
        let expected = sentence
            .get("en")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let exact = output.text.trim() == expected;
        matches += usize::from(exact);
        println!(
            "{}",
            serde_json::json!({"id":id,"source":text,"english":output.text,"expected":expected,"exact":exact,"first_ms":first,"done_ms":started.elapsed().as_millis(),"prompt_tokens":output.prompt_tokens,"cached_tokens":output.cached_tokens,"generated_tokens":output.generated_tokens})
        );
    }
    println!(
        "{}",
        serde_json::json!({"type":"summary","sentences":processed,"exact_matches":matches})
    );
    if let Some(mut supervisor) = supervisor {
        supervisor.stop()?;
    }
    Ok(())
}
#[cfg(not(feature = "llm"))]
fn mt(_: BenchArgs, _: Config, _: Arc<AtomicBool>) -> Result<()> {
    bail!("MT bench requires --features llm");
}
