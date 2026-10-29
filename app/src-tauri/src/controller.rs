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
    pipeline::{Engines, Pipeline, PipelineHandle},
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
    sync::Arc,
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
    /// Starts the translation server if it is not running; answers with its URL.
    EnsureServer(Sender<Result<String, String>>),
    Shutdown(Reply),
}

type Transcript = TranscriptSubscriber<BufWriter<File>>;

/// Everything a listening session owns. The server outlives a pipeline restart, so a settings
/// change that only affects the pipeline keeps it loaded.
#[derive(Default)]
struct Session {
    pipeline: Option<PipelineHandle>,
    transcript: Option<Transcript>,
    supervisor: Option<Supervisor>,
}

pub struct Controller {
    shared: Arc<Shared>,
    session: Session,
}

pub fn spawn(shared: Arc<Shared>, commands: Receiver<Cmd>) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("lt-controller".into())
        .spawn(move || {
            Controller {
                shared,
                session: Session::default(),
            }
            .run(commands)
        })
}

/// Sends a command and waits for the controller's answer.
pub fn request<T>(
    shared: &Shared,
    build: impl FnOnce(Sender<Result<T, String>>) -> Cmd,
) -> Result<T, String> {
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
                Cmd::EnsureServer(reply) => {
                    let config = self.shared.config();
                    let _ = reply.send(self.ensure_server(&config));
                }
                Cmd::Shutdown(reply) => {
                    self.stop_listening();
                    self.stop_server();
                    let _ = reply.send(Ok(()));
                    return;
                }
            }
        }
        self.stop_listening();
        self.stop_server();
    }

    fn start(&mut self) -> Result<(), String> {
        self.drop_finished();
        if let Some(pipeline) = &self.session.pipeline {
            return pipeline.resume().map_err(|e| e.to_string());
        }
        let config = self.shared.config();
        let models = self.shared.paths.models(&config);
        let missing = crate::models::missing_for(&models);
        if !missing.is_empty() {
            return Err(format!(
                "Models are not ready: {}. The control window shows the download.",
                missing.join(", ")
            ));
        }
        let bus = self.shared.bus.clone();
        bus.publish(PipelineEvent::EngineStatus {
            engine: EngineKind::Asr,
            state: EngineState::Loading,
            message: None,
        });
        let (asr, second_pass) = match engines::recognizers(&config, &models) {
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
        self.ensure_server(&config)?;
        let (translator, pid) = engines::translator(
            self.session
                .supervisor
                .as_ref()
                .expect("a start leaves a running supervisor"),
        )?;
        let (source, mode, label) = make_source(&config)?;
        let transcript = self.start_transcript(&config, mode, &label)?;
        let sampler = ProcessSampler::new(pid).map_err(|e| e.to_string())?;
        let pipeline = Pipeline::start(
            config.clone(),
            source,
            Engines {
                asr,
                second_pass,
                translator,
            },
            bus.clone(),
            Some(Box::new(sampler)),
        );
        match pipeline {
            Ok(pipeline) => {
                self.session.pipeline = Some(pipeline);
                self.session.transcript = transcript;
                if config.capture.mode == CaptureMode::Apps && !apps_supported() {
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

    /// The server's URL, after starting it if it is not running.
    fn ensure_server(&mut self, config: &Config) -> Result<String, String> {
        if self
            .session
            .supervisor
            .as_ref()
            .is_some_and(|supervisor| !supervisor.is_running())
        {
            // It gave up after repeated crashes; a new start tries a new server.
            self.stop_server();
        }
        if self.session.supervisor.is_none() {
            let models = self.shared.paths.models(config);
            let (id, file) = lt_core::config::TranslateConfig::MODEL;
            if !models.join(id).join(file).is_file() {
                return Err(
                    "The translation model is not downloaded. The control window shows the download."
                        .into(),
                );
            }
            // Quit sets `quitting` before its command waits here, so a slow model load
            // cannot hold up the exit.
            let bus = &self.shared.bus;
            match engines::start_supervisor(
                config,
                &models,
                &crate::paths::llama_server(),
                bus,
                &self.shared.quitting,
            ) {
                Ok(supervisor) => self.session.supervisor = Some(supervisor),
                Err(message) => {
                    engines::fail(bus, EngineKind::Translator, &message);
                    return Err(message);
                }
            }
        }
        Ok(self
            .session
            .supervisor
            .as_ref()
            .expect("a start leaves a running supervisor")
            .url()
            .to_owned())
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
            asr: lt_core::transcript::recognizer_name(),
            asr_second: lt_core::transcript::SECOND_PASS_NAME.into(),
            translator: lt_core::transcript::translator_name(),
            config: SessionConfig::default(),
        };
        let writer =
            TranscriptWriter::new(BufWriter::new(file), header).map_err(|e| e.to_string())?;
        TranscriptSubscriber::start(
            &self.shared.bus,
            writer,
            Arc::new(|| Local::now().to_rfc3339()),
        )
        .map(Some)
        .map_err(|e| e.to_string())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.drop_finished();
        match &self.session.pipeline {
            Some(pipeline) => pipeline.pause().map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    /// A pipeline ends by itself when its capture fails for good; it cannot resume.
    fn drop_finished(&mut self) {
        if self
            .session
            .pipeline
            .as_ref()
            .is_some_and(PipelineHandle::is_finished)
        {
            self.stop_listening();
        }
    }

    /// Joins the pipeline before the transcript so every terminal event is written.
    fn stop_listening(&mut self) {
        let Some(mut pipeline) = self.session.pipeline.take() else {
            return;
        };
        if let Err(error) = pipeline.stop() {
            tracing::warn!(%error, "Pipeline stopped with an error");
        }
        if let Some(transcript) = self.session.transcript.take() {
            if let Err(error) = transcript.finish().and_then(|mut writer| {
                use std::io::Write;
                writer.flush().map_err(Into::into)
            }) {
                tracing::warn!(%error, "Transcript was not completely written");
            }
        }
        self.shared.bus.publish(PipelineEvent::ListeningState {
            state: ListeningStateKind::Paused,
        });
    }

    fn stop_server(&mut self) {
        if let Some(mut supervisor) = self.session.supervisor.take() {
            if let Err(error) = supervisor.stop() {
                tracing::warn!(%error, "Translator did not stop cleanly");
            }
        }
    }

    fn apply(&mut self, old: &Config, new: &Config) -> Result<(), String> {
        let Some(pipeline) = &self.session.pipeline else {
            if applies::server_changed(old, new) {
                self.stop_server();
            }
            return Ok(());
        };
        match applies::classify(old, new) {
            Applies::Capture => {
                let (source, _, _) = make_source(new)?;
                pipeline.reopen_source(source).map_err(|e| e.to_string())?;
            }
            Applies::Pipeline => {
                let was_listening = self.shared.state().listening == ListeningStateKind::Listening;
                self.stop_listening();
                if applies::server_changed(old, new) {
                    self.stop_server();
                }
                self.start()?;
                if !was_listening {
                    self.pause()?;
                }
                return Ok(());
            }
            Applies::Live | Applies::Restart => {}
        }
        Ok(())
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
    if config.capture.mode == CaptureMode::Apps && apps_supported() {
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

/// The translation server's URL, after starting it if needed. Waits for a start (seconds), so
/// call it from a thread of its own.
pub fn server_url(shared: &Shared) -> Result<String, String> {
    request(shared, Cmd::EnsureServer)
}

pub fn shutdown(shared: &Shared) {
    let _ = request(shared, Cmd::Shutdown);
    lock(&shared.state).listening = ListeningStateKind::Paused;
}
