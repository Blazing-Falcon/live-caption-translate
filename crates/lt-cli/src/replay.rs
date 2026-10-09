use crate::{
    args::{PaceArg, ReplayArgs},
    engines,
    report::ReplayReport,
    wordreport::{self, TimedEvent},
};
use anyhow::{bail, Context, Result};
use chrono::Local;
use lt_audio::{
    resources::ProcessSampler,
    wav::{Pace, WavSource},
};
use lt_core::{
    bus::EventBus,
    config::Config,
    events::PipelineEvent,
    pipeline::{Pipeline, PipelineOptions},
    transcript::{
        SessionConfig, SessionHeader, SessionSource, TranscriptSubscriber, TranscriptWriter,
    },
};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// `<out>` with `.events.jsonl` in place of its extension.
fn sibling(out: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    out.with_extension(suffix)
}

pub fn replay(args: ReplayArgs, mut config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
    if let Some(mode) = args.mode {
        config.latency.mode = mode.config_name().into();
    }
    let models = engines::models_dir(args.models.as_deref(), &config);
    let source = WavSource::new(
        &args.wav,
        match args.pace {
            PaceArg::Realtime => Pace::RealTime,
            PaceArg::Fast => Pace::AsFastAsPossible,
        },
    )
    .with_context(|| format!("Opening WAV {}", args.wav.display()))?
    .with_audio_config(config.audio.clone())
    .with_range(args.start, args.dur)?;
    let error = source.error_handle();
    let info = source.source_info();
    let spec = hound::WavReader::open(&args.wav)?;
    let available = f64::from(spec.duration()) / f64::from(spec.spec().sample_rate);
    let duration = args
        .dur
        .unwrap_or(available)
        .min((available - args.start).max(0.0));
    let bus = EventBus::default();
    let events = bus.subscribe(4096);
    let engines = engines::build(
        &config,
        &models,
        args.server.as_deref(),
        &bus,
        args.fake,
        &cancelled,
    )?;
    if let Some(parent) = args
        .out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let output = BufWriter::new(
        File::create(&args.out).with_context(|| format!("Creating {}", args.out.display()))?,
    );
    let header = SessionHeader {
        app_version: env!("CARGO_PKG_VERSION").into(),
        started_at: Local::now().to_rfc3339(),
        source: SessionSource {
            mode: info.mode,
            label: info.label.clone(),
        },
        asr: if args.fake {
            "fake"
        } else {
            "sensevoice-2024-07-17-int8"
        }
        .into(),
        translator: if args.fake {
            "fake"
        } else {
            "hy-mt2-1.8b-q4_0"
        }
        .into(),
        config: SessionConfig::from(&config).with_draft(engines.draft.is_some()),
    };
    let writer = TranscriptWriter::with_routing(output, header, &config.routing.translate_other)?;
    let subscriber =
        TranscriptSubscriber::start(&bus, writer, Arc::new(|| Local::now().to_rfc3339()))?;
    let mut report = ReplayReport::new(&config.routing);
    let sampler = ProcessSampler::new(engines.pid.clone())?.with_draft(engines.draft_pid.clone());
    let mut timed: Vec<TimedEvent> = Vec::new();
    let events_path = sibling(&args.out, "events.jsonl");
    let mut events_log = BufWriter::new(
        File::create(&events_path)
            .with_context(|| format!("Creating {}", events_path.display()))?,
    );
    let mut pipeline = Pipeline::start_with_options(
        config.clone(),
        Box::new(source),
        engines.vad,
        engines.asr,
        engines.translator,
        bus.clone(),
        PipelineOptions {
            sampler: Some(Box::new(sampler)),
            draft: engines.draft,
            physical_cores: Some(num_cpus::get_physical()),
        },
    )?;
    let session_start = Instant::now();
    let mut record = |event: PipelineEvent, report: &mut ReplayReport| -> Result<()> {
        let t_ms = session_start.elapsed().as_millis() as u64;
        observe(report, &event);
        serde_json::to_writer(
            &mut events_log,
            &serde_json::json!({"t_ms": t_ms, "event": &event}),
        )?;
        events_log.write_all(b"\n")?;
        timed.push(TimedEvent { t_ms, event });
        Ok(())
    };
    while !pipeline.is_finished() {
        if cancelled.load(Ordering::Acquire) {
            pipeline.stop()?;
            break;
        }
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(event) => record(event, &mut report)?,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                bail!("Replay event subscriber disconnected; report is incomplete")
            }
        }
    }
    pipeline.wait()?;
    for event in events.try_iter() {
        record(event, &mut report)?;
    }
    // A closed critical-event subscriber must not produce a false success.
    if matches!(
        events.try_recv(),
        Err(crossbeam_channel::TryRecvError::Disconnected)
    ) {
        bail!("Replay event subscriber overflowed; report is incomplete");
    }
    if let Some(error) = error
        .lock()
        .map_err(|_| anyhow::anyhow!("WAV error observer was interrupted"))?
        .as_ref()
    {
        bail!("WAV replay failed: {error}");
    }
    events_log.flush()?;
    let words = word_metrics(&args, &config, &models, &timed, args.fake);
    let words_path = sibling(&args.out, "words.json");
    fs::write(&words_path, serde_json::to_vec_pretty(&words)?)
        .with_context(|| format!("Writing {}", words_path.display()))?;
    eprintln!("Per-word metrics: {}", serde_json::to_string(&words)?);
    let mut output = subscriber.finish()?;
    let summary = report.summary(pipeline.segment_counts(), &info.label, args.start, duration);
    serde_json::to_writer(&mut output, &summary)?;
    output.write_all(b"\n")?;
    output.flush()?;
    println!("{}", serde_json::to_string(&summary)?);
    #[cfg(feature = "llm")]
    {
        if let Some(mut supervisor) = engines.supervisor {
            supervisor.stop()?;
        }
        if let Some(mut supervisor) = engines.draft_supervisor {
            supervisor.stop()?;
        }
    }
    Ok(())
}

/// Speech instants come from the pipeline's VAD over the replayed range, so the delays are
/// measured from when words are spoken. Without a real VAD (fake engines) there are none.
#[cfg(feature = "sherpa")]
fn word_metrics(
    args: &ReplayArgs,
    config: &Config,
    models: &std::path::Path,
    timed: &[TimedEvent],
    fake: bool,
) -> wordreport::WordMetrics {
    let instants = if fake {
        Vec::new()
    } else {
        (|| -> Result<Vec<u64>> {
            let mut registry = lt_core::engines::EngineRegistry::default();
            lt_sherpa::register_engines(&mut registry);
            let mut engine_config = config.clone();
            engine_config.models.dir = models.to_string_lossy().into_owned();
            let mut vad = registry.build_vad(&engine_config)?;
            let samples = lt_audio::wav::read_wav_mono16(&args.wav)?;
            let from = (args.start * 16_000.0) as usize;
            let to = args
                .dur
                .map_or(samples.len(), |d| from + (d * 16_000.0) as usize)
                .min(samples.len());
            Ok(wordreport::speech_instants(
                &samples[from.min(to)..to],
                vad.as_mut(),
            ))
        })()
        .unwrap_or_else(|error| {
            eprintln!("Speech instants unavailable: {error}");
            Vec::new()
        })
    };
    wordreport::compute(timed, &instants)
}

#[cfg(not(feature = "sherpa"))]
fn word_metrics(
    _: &ReplayArgs,
    _: &Config,
    _: &std::path::Path,
    timed: &[TimedEvent],
    _: bool,
) -> wordreport::WordMetrics {
    wordreport::compute(timed, &[])
}

pub(crate) fn observe(report: &mut ReplayReport, event: &PipelineEvent) {
    report.on_event(event);
    match event {
        PipelineEvent::AsrFinal { id, text, .. } => eprintln!("{} source: {text}", id.0),
        PipelineEvent::TranslationFinal { id, text, .. } => eprintln!("{} English: {text}", id.0),
        PipelineEvent::TranslationFailed {
            id,
            reason,
            message,
        } => eprintln!("{} failed ({reason:?}): {message}", id.0),
        PipelineEvent::EngineStatus {
            state,
            message: Some(message),
            ..
        } => eprintln!("Engine {state:?}: {message}"),
        _ => {}
    }
}
