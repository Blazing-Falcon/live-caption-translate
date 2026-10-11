use super::{Command, FrontendNotice, CONTROL_CAPACITY, POLL};
use crate::{
    bus::EventBus,
    clock::Clock,
    error::{Error, Result},
    events::{ListeningStateKind, PipelineEvent},
    recognizer::RecognizerInput,
    source::{AudioFrame, AudioProducer, AudioReceiver, AudioSource, SessionClock, SourceEvents},
    types::{CutReason, PipelineStats, StreamTime},
};
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

const AUDIO_CAPACITY: usize = 64;
/// The recognizer gets audio in pieces of this size (100 ms).
const FORWARD_SAMPLES: usize = 1_600;

struct Frontend {
    source: Box<dyn AudioSource>,
    input: Option<AudioReceiver>,
    source_cancel: Arc<AtomicBool>,
    inputs: VecDeque<RecognizerInput>,
    notices: VecDeque<FrontendNotice>,
    clock: Arc<SessionClock>,
    bus: EventBus,
    /// The next expected sample. Audio not sent yet starts at `next_at` minus its length.
    next_at: StreamTime,
    pending: Vec<f32>,
    ending: bool,
    started_once: bool,
    listening: Arc<AtomicBool>,
    latest_stats: Arc<Mutex<PipelineStats>>,
}

impl Frontend {
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut self.pending);
        let at = StreamTime(self.next_at.0 - samples.len() as u64);
        self.inputs
            .push_back(RecognizerInput::Audio { at, samples });
    }

    /// The audio stops or jumps: the stream ends its clause.
    fn push_break(&mut self, reason: CutReason) {
        self.flush();
        let at = self.next_at;
        self.inputs.push_back(RecognizerInput::Break { at, reason });
    }

    fn frame(&mut self, frame: AudioFrame) {
        if frame.t0 < self.next_at {
            return;
        }
        if frame.t0 != self.next_at || frame.flags.discontinuity {
            self.push_break(CutReason::Discontinuity);
            self.notices
                .push_back(FrontendNotice::Discontinuity(frame.t0));
            self.next_at = frame.t0;
        }
        self.pending.extend_from_slice(&frame.samples);
        self.next_at = StreamTime(frame.t0.0 + frame.samples.len() as u64);
        if self.pending.len() >= FORWARD_SAMPLES {
            self.flush();
        }
        self.clock.advance_to(self.next_at);
    }

    fn start_source(&mut self) -> Result<()> {
        let at = if self.started_once {
            self.clock.now().max(self.next_at)
        } else {
            StreamTime::ZERO
        };
        self.cut(at);
        let (output, input) = bounded(AUDIO_CAPACITY);
        self.source_cancel = Arc::new(AtomicBool::new(false));
        let producer = AudioProducer::new(output, self.source_cancel.clone(), at)
            .with_session_clock(&self.clock);
        let info = match self
            .source
            .start(producer, SourceEvents::new(self.bus.clone()))
        {
            Ok(info) => info,
            Err(error) => {
                self.stop_source();
                return Err(error);
            }
        };
        self.input = Some(input);
        self.started_once = true;
        self.ending = false;
        self.bus.publish(PipelineEvent::SourceChanged { info });
        self.set_listening(true);
        self.bus.publish(PipelineEvent::ListeningState {
            state: ListeningStateKind::Listening,
        });
        Ok(())
    }

    fn stop_source(&mut self) {
        self.set_listening(false);
        self.source_cancel.store(true, Ordering::Release);
        self.input = None;
        self.source.stop();
    }

    fn set_listening(&self, listening: bool) {
        // Serialize the last Stats publication against the pause boundary.
        let _guard = self.latest_stats.lock().ok();
        self.listening.store(listening, Ordering::Release);
    }

    fn cut(&mut self, at: StreamTime) {
        self.push_break(CutReason::Discontinuity);
        self.notices.push_back(FrontendNotice::Discontinuity(at));
        self.next_at = self.next_at.max(at);
    }

    fn command(&mut self, command: Command) {
        let (reply, result) = match command {
            Command::Pause(reply) => {
                if self.input.is_some() {
                    self.stop_source();
                    self.cut(self.next_at);
                    self.bus.publish(PipelineEvent::ListeningState {
                        state: ListeningStateKind::Paused,
                    });
                }
                (reply, Ok(()))
            }
            Command::Resume(reply) => {
                let result = if self.input.is_none() {
                    self.start_source()
                } else {
                    Ok(())
                };
                (reply, result)
            }
            Command::Reopen(source, reply) => {
                let was_running = self.input.is_some();
                self.stop_source();
                self.cut(self.next_at);
                self.source = source;
                let result = if was_running {
                    self.start_source()
                } else {
                    Ok(())
                };
                (reply, result)
            }
        };
        let _ = reply.try_send(result);
    }
}

impl Drop for Frontend {
    fn drop(&mut self) {
        self.stop_source();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn frontend_loop(
    source: Box<dyn AudioSource>,
    commands: Receiver<Command>,
    audio: Sender<RecognizerInput>,
    notices: Sender<FrontendNotice>,
    clock: Arc<SessionClock>,
    bus: EventBus,
    cancelled: Arc<AtomicBool>,
    ready: Sender<Result<()>>,
    listening: Arc<AtomicBool>,
    latest_stats: Arc<Mutex<PipelineStats>>,
) -> Result<()> {
    let mut frontend = Frontend {
        source,
        input: None,
        source_cancel: Arc::new(AtomicBool::new(false)),
        inputs: VecDeque::new(),
        notices: VecDeque::new(),
        clock,
        bus,
        next_at: StreamTime::ZERO,
        pending: Vec::new(),
        ending: false,
        started_once: false,
        listening,
        latest_stats,
    };
    if let Err(error) = frontend.start_source() {
        let _ = ready.try_send(Err(Error::Engine(error.to_string())));
        cancelled.store(true, Ordering::Release);
        return Err(error);
    }
    let _ = ready.try_send(Ok(()));
    while !cancelled.load(Ordering::Acquire) {
        // Commands stay responsive even if audio is backpressured by ASR.
        for command in commands.try_iter().take(CONTROL_CAPACITY) {
            frontend.command(command);
        }
        while let Some(notice) = frontend.notices.pop_front() {
            match notices.try_send(notice) {
                Ok(()) => {}
                Err(TrySendError::Full(notice)) => {
                    frontend.notices.push_front(notice);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => return Err(Error::Stopped),
            }
        }
        if frontend.notices.is_empty() {
            while let Some(input) = frontend.inputs.pop_front() {
                match audio.try_send(input) {
                    Ok(()) => {}
                    Err(TrySendError::Full(input)) => {
                        frontend.inputs.push_front(input);
                        break;
                    }
                    Err(TrySendError::Disconnected(_)) => return Err(Error::Stopped),
                }
            }
        }
        if frontend.ending && frontend.notices.is_empty() && frontend.inputs.is_empty() {
            break;
        }
        if let Some(notice) = frontend.notices.front().copied() {
            crossbeam_channel::select! {
                send(notices, notice) -> result => {
                    result.map_err(|_| Error::Stopped)?;
                    frontend.notices.pop_front();
                },
                recv(commands) -> command => match command { Ok(command) => frontend.command(command), Err(_) => break },
                default(POLL) => {}
            }
            continue;
        }
        if let Some(input) = frontend.inputs.front().cloned() {
            crossbeam_channel::select! {
                send(audio, input) -> result => {
                    result.map_err(|_| Error::Stopped)?;
                    frontend.inputs.pop_front();
                },
                recv(commands) -> command => match command { Ok(command) => frontend.command(command), Err(_) => break },
                default(POLL) => {}
            }
            continue;
        }
        let input = frontend
            .input
            .clone()
            .unwrap_or_else(crossbeam_channel::never);
        crossbeam_channel::select! {
            recv(commands) -> command => match command { Ok(command) => frontend.command(command), Err(_) => break },
            recv(input) -> frame => match frame {
                Ok(frame) => frontend.frame(frame),
                Err(_) => {
                    frontend.stop_source();
                    frontend.push_break(CutReason::End);
                    frontend.ending = true;
                    // The audio ended by itself (end of a file, or a capture that failed for
                    // good): listening is over, and nothing else would say so.
                    frontend.bus.publish(PipelineEvent::ListeningState {
                        state: ListeningStateKind::Paused,
                    });
                }
            },
            default(POLL) => {}
        }
    }
    Ok(())
}
