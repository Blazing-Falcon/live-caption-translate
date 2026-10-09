use crate::args::LiveArgs;
use anyhow::{bail, Result};
use lt_core::config::Config;
use std::sync::{atomic::AtomicBool, Arc};

#[cfg(windows)]
pub fn live(args: LiveArgs, mut config: Config, cancelled: Arc<AtomicBool>) -> Result<()> {
    use crate::{args::CaptureModeArg, engines};
    use lt_audio::windows::{apps, AppsCaptureSource, DeviceChoice, LoopbackSource};
    use lt_core::{config::CaptureApp, source::AudioSource, types::CaptureMode};

    let models = engines::models_dir(args.models.as_deref(), &config);
    let requested_apps = matches!(args.mode, CaptureModeArg::Apps);
    if !args.app.is_empty() {
        config.capture.apps = args
            .app
            .iter()
            .map(|exe| CaptureApp {
                exe: exe.clone(),
                name: exe.clone(),
                ..CaptureApp::default()
            })
            .collect();
    }
    let supported = !requested_apps || apps::process_loopback_supported(apps::windows_build()?);
    let (source, mode, label): (Box<dyn AudioSource>, _, _) = if requested_apps && supported {
        (
            Box::new(AppsCaptureSource::new(
                config.capture.apps.clone(),
                config.audio.clone(),
            )?),
            CaptureMode::Apps,
            "Selected apps",
        )
    } else {
        if requested_apps {
            eprintln!("Selected apps requires Windows build 20348 or later; using whole system");
        }
        let device = if config.capture.device == "default" {
            DeviceChoice::FollowDefault
        } else {
            DeviceChoice::Id(config.capture.device.clone())
        };
        (
            Box::new(LoopbackSource::new(device, config.audio.clone())?),
            CaptureMode::System,
            "Whole system",
        )
    };
    let bus = lt_core::bus::EventBus::default();
    let engines = engines::build(
        &config,
        &models,
        args.server.as_deref(),
        &bus,
        false,
        &cancelled,
    )?;
    run_session(args, config, cancelled, source, mode, label, bus, engines)
}

#[cfg(not(windows))]
pub fn live(_: LiveArgs, _: Config, _: Arc<AtomicBool>) -> Result<()> {
    bail!("Live audio capture requires Windows; use replay for file input")
}

#[cfg(any(windows, test))]
#[allow(clippy::too_many_arguments)]
fn run_session(
    args: LiveArgs,
    config: Config,
    cancelled: Arc<AtomicBool>,
    source: Box<dyn lt_core::source::AudioSource>,
    mode: lt_core::types::CaptureMode,
    label: &str,
    bus: lt_core::bus::EventBus,
    engines: crate::engines::Engines,
) -> Result<()> {
    use crate::{replay::observe, report::ReplayReport};
    use anyhow::Context;
    use chrono::Local;
    use lt_audio::resources::ProcessSampler;
    use lt_core::{
        events::PipelineEvent,
        pipeline::Pipeline,
        transcript::{
            SessionConfig, SessionHeader, SessionSource, TranscriptSubscriber, TranscriptWriter,
        },
    };
    use std::{
        fs::{self, File},
        io::{BufWriter, Write},
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };

    let events = bus.subscribe(4096);
    let destination = args.out.or_else(|| {
        config.transcript.enabled.then(|| {
            std::path::PathBuf::from("out").join(format!(
                "live-{}.jsonl",
                Local::now().format("%Y%m%d-%H%M%S")
            ))
        })
    });
    let subscriber = if let Some(path) = &destination {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let header = SessionHeader {
            app_version: env!("CARGO_PKG_VERSION").into(),
            started_at: Local::now().to_rfc3339(),
            source: SessionSource {
                mode,
                label: label.into(),
            },
            asr: config.asr.engine.clone(),
            translator: config.translate.engine.clone(),
            config: SessionConfig::from(&config).with_draft(engines.draft.is_some()),
        };
        let writer = TranscriptWriter::with_routing(
            BufWriter::new(
                File::create(path).with_context(|| format!("Creating {}", path.display()))?,
            ),
            header,
            &config.routing.translate_other,
        )?;
        Some(TranscriptSubscriber::start(
            &bus,
            writer,
            Arc::new(|| Local::now().to_rfc3339()),
        )?)
    } else {
        None
    };
    let mut report = ReplayReport::new(&config.routing);
    let sampler = ProcessSampler::new(engines.pid.clone())?.with_draft(engines.draft_pid.clone());
    let mut pipeline = Pipeline::start_with_options(
        config,
        source,
        engines.vad,
        engines.asr,
        engines.translator,
        bus.clone(),
        lt_core::pipeline::PipelineOptions {
            sampler: Some(Box::new(sampler)),
            draft: engines.draft,
            physical_cores: Some(num_cpus::get_physical()),
        },
    )?;
    let started = Instant::now();
    while !pipeline.is_finished() {
        if cancelled.load(Ordering::Acquire)
            || args
                .dur
                .is_some_and(|duration| started.elapsed().as_secs_f64() >= duration)
        {
            pipeline.stop()?;
            break;
        }
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(event) => {
                if let PipelineEvent::SourceState { state, detail } = &event {
                    eprintln!(
                        "Source {state:?}: {}",
                        detail.as_deref().unwrap_or_default()
                    );
                }
                observe(&mut report, &event);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                bail!("Live event subscriber disconnected; report is incomplete")
            }
        }
    }
    pipeline.wait()?;
    for event in events.try_iter() {
        observe(&mut report, &event);
    }
    if matches!(
        events.try_recv(),
        Err(crossbeam_channel::TryRecvError::Disconnected)
    ) {
        bail!("Live event subscriber overflowed; report is incomplete");
    }
    let summary = report.summary(
        pipeline.segment_counts(),
        label,
        0.0,
        started.elapsed().as_secs_f64(),
    );
    if let Some(subscriber) = subscriber {
        let mut writer = subscriber.finish()?;
        serde_json::to_writer(&mut writer, &summary)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{args::CaptureModeArg, engines};
    use lt_audio::wav::{Pace, WavSource};
    use lt_core::{bus::EventBus, types::CaptureMode};
    use std::path::PathBuf;

    #[test]
    fn live_session_lifecycle_can_replay_without_touching_native_devices() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let workspace = workspace.canonicalize().unwrap();
        let scratch = workspace.join(format!("target/tmp/live-session-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let file = scratch.join("speech.wav");
        let output = scratch.join("live.jsonl");
        let mut wav = hound::WavWriter::create(
            &file,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for at in 0..32_000 {
            let sample = if (3_200..16_000).contains(&at) {
                (0.3 * (std::f64::consts::TAU * 440.0 * f64::from(at) / 16_000.0).sin() * 32767.0)
                    as i16
            } else {
                0
            };
            wav.write_sample(sample).unwrap();
        }
        wav.finalize().unwrap();
        let mut config = Config::default();
        config.transcript.enabled = false;
        config.audio.normalize = false;
        config.join.hold_max_chars = 0;
        let source = WavSource::new(file, Pace::AsFastAsPossible).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let bus = EventBus::default();
        let engines = engines::build(
            &config,
            &workspace.join("models"),
            None,
            &bus,
            true,
            &cancelled,
        )
        .unwrap();
        run_session(
            LiveArgs {
                mode: CaptureModeArg::System,
                app: Vec::new(),
                config: None,
                models: None,
                server: None,
                dur: None,
                out: Some(output.clone()),
            },
            config,
            cancelled,
            Box::new(source),
            CaptureMode::System,
            "Synthetic system capture",
            bus,
            engines,
        )
        .unwrap();
        let records = std::fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records[0]["type"], "session");
        let summary = records.last().unwrap();
        assert_eq!(summary["type"], "summary");
        assert_eq!(summary["segments"], 1);
        assert_eq!(summary["status"]["final"], 1);
        assert_eq!(summary["status"]["failed"], 0);
        let target = scratch.canonicalize().unwrap();
        assert!(target.starts_with(workspace.join("target/tmp")));
        std::fs::remove_dir_all(target).unwrap();
    }
}
