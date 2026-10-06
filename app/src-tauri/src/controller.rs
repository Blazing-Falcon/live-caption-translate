//! One thread owns the pipeline, the translator process and the transcript writer, so slow
//! work (model loading, joining native threads) never runs under a lock or on the UI thread.
use crate::{
    app::{lock, Shared},
    applies::{self, Applies},
    engines,
};
use chrono::Local;
use crossbeam_channel::{unbounded, Receiver, Sender};
use lt_audio::{
    resources::ProcessSampler,
    wav::{Pace, WavSource},
};
use lt_core::{
    config::Config,
    events::{EngineKind, EngineState, ListeningStateKind, PipelineEvent, SourceStateKind},
    pipeline::{Pipeline, PipelineHandle},
    source::AudioSource,
    transcript::{
        SessionConfig, SessionHeader, SessionSource, TranscriptSubscriber, TranscriptWriter,
    },
    types::CaptureMode,
};
use lt_llm::supervisor::Supervisor;
use std::{
    fs::File,
    io::BufWriter,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

pub type Reply = Sender<Result<(), String>>;

#[allow(clippy::large_enum_variant)]
pub enum Cmd {
    Start(Reply),
    Pause(Reply),
    /// Hotkey and tray toggle: resumes when paused or stopped, otherwise pauses.
    TogglePause,
    Apply {
        old: Config,
        new: Config,
    },
    Shutdown(Reply),
}

type Transcript = TranscriptSubscriber<BufWriter<File>>;

struct Session {
    pipeline: PipelineHandle,
    transcript: Option<Transcript>,
}

pub struct Controller {
    shared: Arc<Shared>,
    session: Option<Session>,
    supervisor: Option<Supervisor>,
    cancelled: Arc<AtomicBool>,
}

pub fn spawn(shared: Arc<Shared>, commands: Receiver<Cmd>) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("lt-controller".into())
        .spawn(move || {
            Controller {
                shared,
                session: None,
                supervisor: None,
                cancelled: Arc::new(AtomicBool::new(false)),
            }
            .run(commands)
        })
}

/// Sends a command and waits for the controller's answer.
pub fn request(shared: &Shared, build: impl FnOnce(Reply) -> Cmd) -> Result<(), String> {
    let (reply, answer) = unbounded();
    shared
        .controller
        .send(build(reply))
        .map_err(|_| "The app is shutting down".to_string())?;
    answer
        .recv()
        .map_err(|_| "The app is shutting down".to_string())?
}

impl Controller {
    fn run(mut self, commands: Receiver<Cmd>) {
        while let Ok(command) = commands.recv() {
            match command {
                Cmd::Start(reply) => {
                    let _ = reply.send(self.start());
                }
                Cmd::Pause(reply) => {
                    let _ = reply.send(self.pause());
                }
                Cmd::TogglePause => {
                    let listening = self.shared.state().listening == ListeningStateKind::Listening;
                    let result = if listening {
                        self.pause()
                    } else {
                        self.start()
                    };
                    if let Err(message) = result {
                        tracing::warn!(message, "Pause toggle failed");
                    }
                }
                Cmd::Apply { old, new } => {
                    if let Err(message) = self.apply(&old, &new) {
                        tracing::warn!(message, "Applying settings failed");
                    }
                }
                Cmd::Shutdown(reply) => {
                    self.cancelled.store(true, Ordering::Release);
                    self.stop_session();
                    self.stop_supervisor();
                    let _ = reply.send(Ok(()));
                    return;
                }
            }
        }
        self.stop_session();
        self.stop_supervisor();
    }

    fn start(&mut self) -> Result<(), String> {
        if let Some(session) = &self.session {
            return session.pipeline.resume().map_err(|e| e.to_string());
        }
        let config = self.shared.config();
        let models = self.shared.paths.models(&config);
        if !crate::models::all_ready(&models) {
            return Err("Models are not downloaded yet. Open settings to download them.".into());
        }
        self.cancelled.store(false, Ordering::Release);
        let bus = self.shared.bus.clone();
        for engine in [EngineKind::Vad, EngineKind::Asr] {
            bus.publish(PipelineEvent::EngineStatus {
                engine,
                state: EngineState::Loading,
                message: None,
            });
        }
        let (vad, asr) = match engines::speech(&config, &models) {
            Ok(loaded) => loaded,
            Err(message) => {
                engines::fail(&bus, EngineKind::Asr, &message);
                return Err(format!("Speech recognition could not start: {message}"));
            }
        };
        bus.publish(PipelineEvent::EngineStatus {
            engine: EngineKind::Asr,
            state: EngineState::Ready,
            message: None,
        });
        if config.translate.server_url.is_empty() && self.supervisor.is_none() {
            match engines::start_supervisor(
                &config,
                &models,
                &crate::paths::llama_server(),
                &bus,
                &self.cancelled,
            ) {
                Ok(supervisor) => self.supervisor = Some(supervisor),
                Err(message) => {
                    engines::fail(&bus, EngineKind::Translator, &message);
                    return Err(message);
                }
            }
        }
        let (translator, pid) = engines::translator(&config, self.supervisor.as_ref())?;
        let (source, mode, label) = make_source(&config)?;
        let transcript = self.start_transcript(&config, mode, &label)?;
        let sampler = ProcessSampler::new(pid).map_err(|e| e.to_string())?;
        let pipeline = Pipeline::start_with_bus_and_sampler(
            config.clone(),
            source,
            vad,
            asr,
            translator,
            bus.clone(),
            Some(Box::new(sampler)),
        );
        match pipeline {
            Ok(pipeline) => {
                self.session = Some(Session {
                    pipeline,
                    transcript,
                });
                if config.capture.mode == "apps" && !apps_supported() {
                    bus.publish(PipelineEvent::SourceState {
                        state: SourceStateKind::Unsupported,
                        detail: Some("Selected apps needs Windows 11".into()),
                    });
                }
                Ok(())
            }
            Err(error) => {
                if let Some(transcript) = transcript {
                    let _ = transcript.finish();
                }
                Err(format!("Listening could not start: {error}"))
            }
        }
    }

    fn start_transcript(
        &self,
        config: &Config,
        mode: CaptureMode,
        label: &str,
    ) -> Result<Option<Transcript>, String> {
        if !config.transcript.enabled {
            return Ok(None);
        }
        let dir = self.shared.paths.transcripts();
        std::fs::create_dir_all(&dir).map_err(|e| format!("Transcript folder: {e}"))?;
        let path = dir.join(format!("{}.jsonl", Local::now().format("%Y-%m-%d_%H%M%S")));
        let file = File::create(&path).map_err(|e| format!("Transcript file: {e}"))?;
        let header = SessionHeader {
            app_version: env!("CARGO_PKG_VERSION").into(),
            started_at: Local::now().to_rfc3339(),
            source: SessionSource {
                mode,
                label: label.into(),
            },
            asr: config.asr.engine.clone(),
            translator: config.translate.engine.clone(),
            config: SessionConfig::from(config),
        };
        let writer = TranscriptWriter::with_routing(
            BufWriter::new(file),
            header,
            &config.routing.translate_other,
        )
        .map_err(|e| e.to_string())?;
        TranscriptSubscriber::start(
            &self.shared.bus,
            writer,
            Arc::new(|| Local::now().to_rfc3339()),
        )
        .map(Some)
        .map_err(|e| e.to_string())
    }

    fn pause(&mut self) -> Result<(), String> {
        match &self.session {
            Some(session) => session.pipeline.pause().map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    /// Joins the pipeline before the transcript so every terminal event is written.
    fn stop_session(&mut self) {
        if let Some(mut session) = self.session.take() {
            if let Err(error) = session.pipeline.stop() {
                tracing::warn!(%error, "Pipeline stopped with an error");
            }
            if let Some(transcript) = session.transcript.take() {
                if let Err(error) = transcript.finish().and_then(|mut w| {
                    use std::io::Write;
                    w.flush().map_err(Into::into)
                }) {
                    tracing::warn!(%error, "Transcript was not completely written");
                }
            }
            self.shared.bus.publish(PipelineEvent::ListeningState {
                state: ListeningStateKind::Paused,
            });
        }
    }

    fn stop_supervisor(&mut self) {
        if let Some(mut supervisor) = self.supervisor.take() {
            if let Err(error) = supervisor.stop() {
                tracing::warn!(%error, "Translator did not stop cleanly");
            }
        }
    }

    fn apply(&mut self, old: &Config, new: &Config) -> Result<(), String> {
        let Some(session) = &self.session else {
            if applies::server_changed(old, new) {
                self.stop_supervisor();
            }
            return Ok(());
        };
        match applies::classify(old, new) {
            Applies::Capture => {
                let (source, _, _) = make_source(new)?;
                session
                    .pipeline
                    .reopen_source(source)
                    .map_err(|e| e.to_string())
            }
            Applies::Pipeline => {
                let was_listening = self.shared.state().listening == ListeningStateKind::Listening;
                self.stop_session();
                if applies::server_changed(old, new) {
                    self.stop_supervisor();
                }
                self.start()?;
                if !was_listening {
                    self.pause()?;
                }
                Ok(())
            }
            Applies::Live | Applies::Restart => Ok(()),
        }
    }
}

fn apps_supported() -> bool {
    lt_audio::windows::apps::windows_build()
        .map(lt_audio::windows::apps::process_loopback_supported)
        .unwrap_or(false)
}

/// The development `LT_REPLAY=file.wav` variable replays a file instead of capturing audio.
fn make_source(config: &Config) -> Result<(Box<dyn AudioSource>, CaptureMode, String), String> {
    use lt_audio::windows::{AppsCaptureSource, DeviceChoice, LoopbackSource};
    if let Some(path) = std::env::var_os("LT_REPLAY").filter(|path| !path.is_empty()) {
        let source = WavSource::new(path, Pace::RealTime).map_err(|e| e.to_string())?;
        return Ok((Box::new(source), CaptureMode::System, "Replay".into()));
    }
    if config.capture.mode == "apps" && apps_supported() {
        let names: Vec<_> = config
            .capture
            .apps
            .iter()
            .map(|app| {
                if app.name.is_empty() {
                    &app.exe
                } else {
                    &app.name
                }
            })
            .cloned()
            .collect();
        let source = AppsCaptureSource::new(config.capture.apps.clone(), config.audio.clone())
            .map_err(|e| e.to_string())?;
        return Ok((Box::new(source), CaptureMode::Apps, names.join(", ")));
    }
    let device = if config.capture.device == "default" {
        DeviceChoice::FollowDefault
    } else {
        DeviceChoice::Id(config.capture.device.clone())
    };
    let source = LoopbackSource::new(device, config.audio.clone()).map_err(|e| e.to_string())?;
    Ok((Box::new(source), CaptureMode::System, "System audio".into()))
}

pub fn shutdown(shared: &Shared) {
    let _ = request(shared, Cmd::Shutdown);
    lock(&shared.state).listening = ListeningStateKind::Paused;
}
