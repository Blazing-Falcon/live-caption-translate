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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::{path::PathBuf, sync::atomic::AtomicU64, thread, time::Instant};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap();
            let path = workspace.join(format!(
                "target/tmp/cli-cancel-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap();
            let target = self.0.canonicalize().unwrap();
            assert!(target.starts_with(workspace.join("target/tmp")));
            fs::remove_dir_all(target).unwrap();
        }
    }

    #[test]
    fn simulated_signal_stops_realtime_source_and_flushes_prior_terminal_records() {
        let scratch = Scratch::new();
        let wav = scratch.0.join("speech.wav");
        let out = scratch.0.join("cancelled.jsonl");
        let mut writer = hound::WavWriter::create(
            &wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for at in 0..128_000 {
            let speech = (1600..9600).contains(&at) || at >= 22_400;
            let value = if speech {
                (0.3 * (std::f64::consts::TAU * 440.0 * f64::from(at) / 16_000.0).sin() * 32767.0)
                    as i16
            } else {
                0
            };
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
        let mut config = Config::default();
        config.audio.normalize = false;
        config.join.hold_max_chars = 0;
        let args = ReplayArgs {
            wav,
            start: 0.0,
            dur: None,
            pace: PaceArg::Realtime,
            out: out.clone(),
            config: None,
            models: None,
            server: None,
            fake: true,
            mode: None,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let signal_out = out.clone();
        let signal = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !signal_out.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(signal_out.exists());
            thread::sleep(Duration::from_millis(1600));
            let at = Instant::now();
            flag.store(true, Ordering::Release);
            at
        });
        replay(args, config, cancelled).unwrap();
        assert!(signal.join().unwrap().elapsed() < Duration::from_secs(1));
        let records: Vec<Value> = fs::read_to_string(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.first().unwrap()["type"], "session");
        assert!(
            records
                .iter()
                .any(|record| record["type"] == "utterance" && record["english"] == "Hello."),
            "{records:?}"
        );
        assert_eq!(records.last().unwrap()["type"], "summary");
        assert_eq!(records.last().unwrap()["status"]["final"], 1);
    }
}
