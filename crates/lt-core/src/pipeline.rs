//! Bounded, independently scheduled audio/ASR/translation workers.
//!
//! Audio adapters deliver prepared 16 kHz frames. Engines remain owned by one
//! worker each; source cancellation and whole-pipeline shutdown are separate.

use crate::{
    bus::{EventBus, EventReceiver},
    clock::{Clock, ManualClock},
    config::Config,
    engines::{StreamingAsr, Translator},
    error::{Error, Result},
    events::{DropReason, EngineKind, EngineState, FailReason, ListeningStateKind, PipelineEvent},
    join::{JoinResult, Joiner},
    metrics::{Metrics, ResourceSample, ResourceSampler},
    queue::{TranslationQueue, TranslatorWorker},
    source::{AudioFrame, AudioProducer, AudioReceiver, AudioSource, SessionClock, SourceEvents},
    stream::{AsrMessage, StreamInput, StreamOutput, StreamRecognizer},
    types::{CutReason, PipelineStats, StreamTime, TextClass, Transcript, UtteranceId},
};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(5);
const AUDIO_CAPACITY: usize = 64;
const CONTROL_CAPACITY: usize = 16;
const ASR_CAPACITY: usize = 4;
/// The recognizer gets audio in pieces of this size (100 ms).
const STREAM_FORWARD_SAMPLES: usize = 1_600;

enum Command {
    Pause(Sender<Result<()>>),
    Resume(Sender<Result<()>>),
    Reopen(Box<dyn AudioSource>, Sender<Result<()>>),
}

#[derive(Clone, Copy)]
enum FrontendNotice {
    Started(UtteranceId, StreamTime),
    Discontinuity(StreamTime),
    Progress(StreamTime),
    SegmentClosed(UtteranceId, StreamTime),
}

pub struct Pipeline;

/// Aggregate frontend diagnostics for replay reports; this does not change IPC.
#[derive(Clone, Copy, Debug, Default)]
pub struct SegmentCounts {
    pub pause: u64,
    pub discontinuity: u64,
    pub end: u64,
    pub commit: u64,
}

impl SegmentCounts {
    pub fn total(self) -> u64 {
        self.pause + self.discontinuity + self.end + self.commit
    }
}

#[derive(Default)]
struct SegmentCounters([AtomicU64; 4]);

impl SegmentCounters {
    fn record(&self, reason: CutReason) {
        let index = match reason {
            CutReason::Pause => 0,
            CutReason::Discontinuity => 1,
            CutReason::End => 2,
            CutReason::Commit => 3,
        };
        self.0[index].fetch_add(1, Ordering::Relaxed);
    }
    fn snapshot(&self) -> SegmentCounts {
        let counts = self.0.each_ref().map(|count| count.load(Ordering::Relaxed));
        SegmentCounts {
            pause: counts[0],
            discontinuity: counts[1],
            end: counts[2],
            commit: counts[3],
        }
    }
}

pub struct PipelineHandle {
    commands: Sender<Command>,
    cancelled: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    bus: EventBus,
    threads: Vec<JoinHandle<Result<()>>>,
    latest_stats: Arc<Mutex<PipelineStats>>,
    segment_counts: Arc<SegmentCounters>,
}

/// Optional parts of a pipeline.
#[derive(Default)]
pub struct PipelineOptions {
    pub sampler: Option<Box<dyn ResourceSampler>>,
}

impl Pipeline {
    pub fn start(
        config: Config,
        source: Box<dyn AudioSource>,
        asr: Box<dyn StreamingAsr>,
        translator: Box<dyn Translator>,
    ) -> Result<PipelineHandle> {
        Self::start_with_bus(config, source, asr, translator, EventBus::default())
    }

    /// Subscribe before calling this method when replay can finish immediately.
    pub fn start_with_bus(
        config: Config,
        source: Box<dyn AudioSource>,
        asr: Box<dyn StreamingAsr>,
        translator: Box<dyn Translator>,
        bus: EventBus,
    ) -> Result<PipelineHandle> {
        Self::start_with_bus_and_sampler(config, source, asr, translator, bus, None)
    }

    pub fn start_with_bus_and_sampler(
        config: Config,
        source: Box<dyn AudioSource>,
        asr: Box<dyn StreamingAsr>,
        translator: Box<dyn Translator>,
        bus: EventBus,
        sampler: Option<Box<dyn ResourceSampler>>,
    ) -> Result<PipelineHandle> {
        Self::start_with_options(
            config,
            source,
            asr,
            translator,
            bus,
            PipelineOptions { sampler },
        )
    }

    pub fn start_with_options(
        mut config: Config,
        source: Box<dyn AudioSource>,
        asr: Box<dyn StreamingAsr>,
        translator: Box<dyn Translator>,
        bus: EventBus,
        options: PipelineOptions,
    ) -> Result<PipelineHandle> {
        let PipelineOptions { sampler } = options;
        for message in config.validate() {
            tracing::warn!(message, "Pipeline config adjusted");
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let clock = Arc::new(SessionClock::default());
        let listening = Arc::new(AtomicBool::new(false));
        let latest_stats = Arc::new(Mutex::new(PipelineStats::default()));
        let segment_counts = Arc::new(SegmentCounters::default());
        let (command_tx, command_rx) = bounded(CONTROL_CAPACITY);
        let (audio_tx, audio_rx) = bounded(ASR_CAPACITY);
        let (front_tx, front_rx) = bounded(CONTROL_CAPACITY);
        let (asr_tx, asr_rx) = bounded(ASR_CAPACITY);
        let (ready_tx, ready_rx) = bounded(1);
        let mut handle = PipelineHandle {
            commands: command_tx,
            cancelled: cancelled.clone(),
            finished: finished.clone(),
            bus: bus.clone(),
            threads: Vec::with_capacity(3),
            latest_stats: latest_stats.clone(),
            segment_counts: segment_counts.clone(),
        };
        bus.publish(PipelineEvent::ListeningState {
            state: ListeningStateKind::Starting,
        });

        let asr_cancel = cancelled.clone();
        let asr_bus = bus.clone();
        let asr_config = config.clone();
        let asr_notices = AsrNotices {
            notices: front_tx.clone(),
            segment_counts: segment_counts.clone(),
        };
        let asr_thread = thread::Builder::new()
            .name("lt-asr".into())
            .spawn(move || {
                asr_loop(
                    asr,
                    &asr_config,
                    audio_rx,
                    asr_tx,
                    &asr_bus,
                    &asr_cancel,
                    asr_notices,
                )
            })?;
        handle.threads.push(asr_thread);

        let scheduler_cancel = cancelled.clone();
        let scheduler_clock = clock.clone();
        let scheduler_bus = bus.clone();
        let scheduler_config = config.clone();
        let scheduler_listening = listening.clone();
        let scheduler_latest = latest_stats.clone();
        let scheduler_thread =
            thread::Builder::new()
                .name("lt-scheduler".into())
                .spawn(move || {
                    let _finished = Finished(finished);
                    scheduler_loop(
                        scheduler_config,
                        translator,
                        front_rx,
                        asr_rx,
                        scheduler_clock,
                        scheduler_bus,
                        scheduler_cancel,
                        scheduler_listening,
                        scheduler_latest,
                        sampler,
                    )
                })?;
        handle.threads.push(scheduler_thread);

        let front_cancel = cancelled;
        let front_bus = bus;
        let frontend_thread =
            thread::Builder::new()
                .name("lt-frontend".into())
                .spawn(move || {
                    frontend_loop(
                        source,
                        command_rx,
                        audio_tx,
                        front_tx,
                        clock,
                        front_bus,
                        front_cancel,
                        ready_tx,
                        listening,
                        latest_stats,
                    )
                })?;
        handle.threads.insert(0, frontend_thread);

        loop {
            match ready_rx.recv_timeout(POLL) {
                Ok(Ok(())) => return Ok(handle),
                Ok(Err(error)) => return Err(error),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if handle.is_finished() {
                        return Err(Error::Stopped);
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return Err(Error::Stopped)
                }
            }
        }
    }
}

impl PipelineHandle {
    pub fn subscribe(&self, capacity: usize) -> EventReceiver {
        self.bus.subscribe(capacity)
    }

    pub fn get_stats(&self) -> PipelineStats {
        self.latest_stats
            .lock()
            .map(|stats| stats.clone())
            .unwrap_or_default()
    }

    pub fn segment_counts(&self) -> SegmentCounts {
        self.segment_counts.snapshot()
    }

    pub fn pause(&self) -> Result<()> {
        let (reply, response) = bounded(1);
        self.command(Command::Pause(reply), response)
    }

    pub fn resume(&self) -> Result<()> {
        let (reply, response) = bounded(1);
        self.command(Command::Resume(reply), response)
    }

    /// Replace the capture adapter. A paused pipeline stays paused until resume.
    pub fn reopen_source(&self, source: Box<dyn AudioSource>) -> Result<()> {
        let (reply, response) = bounded(1);
        self.command(Command::Reopen(source, reply), response)
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Drain a finite replay naturally, including all pending ASR and translation.
    pub fn wait(&mut self) -> Result<()> {
        self.join_threads()
    }

    /// Built-in sources/translators cooperate within their polling intervals.
    /// A native ASR call must return before its worker can join; it has no force
    /// cancellation API and the pipeline never abandons that thread.
    pub fn stop(&mut self) -> Result<()> {
        self.cancelled.store(true, Ordering::Release);
        self.join_threads()
    }

    fn command(&self, mut command: Command, response: Receiver<Result<()>>) -> Result<()> {
        loop {
            if self.cancelled.load(Ordering::Acquire) || self.is_finished() {
                return Err(Error::Stopped);
            }
            match self.commands.send_timeout(command, POLL) {
                Ok(()) => break,
                Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => command = returned,
                Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                    return Err(Error::Stopped)
                }
            }
        }
        loop {
            match response.recv_timeout(POLL) {
                Ok(result) => return result,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if self.cancelled.load(Ordering::Acquire) || self.is_finished() {
                        return Err(Error::Stopped);
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return Err(Error::Stopped)
                }
            }
        }
    }

    fn join_threads(&mut self) -> Result<()> {
        let mut first_error = None;
        for thread in self.threads.drain(..) {
            let result = thread.join().unwrap_or_else(|_| {
                Err(Error::Engine("Pipeline worker stopped unexpectedly".into()))
            });
            if let Err(error) = result {
                if !matches!(error, Error::Stopped) && first_error.is_none() {
                    first_error = Some(error);
                }
                self.cancelled.store(true, Ordering::Release);
            }
        }
        self.finished.store(true, Ordering::Release);
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct Finished(Arc<AtomicBool>);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct Frontend {
    source: Box<dyn AudioSource>,
    input: Option<AudioReceiver>,
    source_cancel: Arc<AtomicBool>,
    inputs: VecDeque<StreamInput>,
    notices: VecDeque<FrontendNotice>,
    clock: Arc<SessionClock>,
    bus: EventBus,
    /// The next expected sample. Audio not sent yet starts at `stream_at` minus its length.
    stream_at: StreamTime,
    stream_pending: Vec<f32>,
    ending: bool,
    started_once: bool,
    listening: Arc<AtomicBool>,
    latest_stats: Arc<Mutex<PipelineStats>>,
}

impl Frontend {
    fn flush_stream(&mut self) {
        if self.stream_pending.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut self.stream_pending);
        let at = StreamTime(self.stream_at.0 - samples.len() as u64);
        self.inputs.push_back(StreamInput::Audio { at, samples });
    }

    /// The audio stops or jumps: the stream ends its clause.
    fn stream_break(&mut self, reason: CutReason) {
        self.flush_stream();
        let at = self.stream_at;
        self.inputs.push_back(StreamInput::Break { at, reason });
    }

    fn frame(&mut self, frame: AudioFrame) {
        if frame.t0 < self.stream_at {
            return;
        }
        if frame.t0 != self.stream_at || frame.flags.discontinuity {
            self.stream_break(CutReason::Discontinuity);
            self.notices
                .push_back(FrontendNotice::Discontinuity(frame.t0));
            self.stream_at = frame.t0;
        }
        self.stream_pending.extend_from_slice(&frame.samples);
        self.stream_at = StreamTime(frame.t0.0 + frame.samples.len() as u64);
        if self.stream_pending.len() >= STREAM_FORWARD_SAMPLES {
            self.flush_stream();
        }
        self.clock.advance_to(self.stream_at);
        self.notices
            .push_back(FrontendNotice::Progress(self.stream_at));
    }

    fn start_source(&mut self) -> Result<()> {
        let at = if self.started_once {
            self.clock.now().max(self.stream_at)
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
        self.stream_break(CutReason::Discontinuity);
        self.notices.push_back(FrontendNotice::Discontinuity(at));
        self.stream_at = self.stream_at.max(at);
    }

    fn command(&mut self, command: Command) {
        let (reply, result) = match command {
            Command::Pause(reply) => {
                if self.input.is_some() {
                    self.stop_source();
                    self.cut(self.stream_at);
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
                self.cut(self.stream_at);
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
fn frontend_loop(
    source: Box<dyn AudioSource>,
    commands: Receiver<Command>,
    audio: Sender<StreamInput>,
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
        stream_at: StreamTime::ZERO,
        stream_pending: Vec::new(),
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
                    frontend.stream_break(CutReason::End);
                    frontend.ending = true;
                }
            },
            default(POLL) => {}
        }
    }
    Ok(())
}

/// What the recognizer needs to report clauses itself.
struct AsrNotices {
    notices: Sender<FrontendNotice>,
    segment_counts: Arc<SegmentCounters>,
}

fn asr_loop(
    mut asr: Box<dyn StreamingAsr>,
    config: &Config,
    inputs: Receiver<StreamInput>,
    output: Sender<AsrMessage>,
    bus: &EventBus,
    cancelled: &AtomicBool,
    stream: AsrNotices,
) -> Result<()> {
    bus.publish(PipelineEvent::EngineStatus {
        engine: EngineKind::Asr,
        state: EngineState::Ready,
        message: None,
    });
    let mut recognizer = StreamRecognizer::new(config);
    let mut produced = Vec::new();
    while !cancelled.load(Ordering::Acquire) {
        let input = match inputs.recv_timeout(POLL) {
            Ok(input) => input,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        recognizer.handle(asr.as_mut(), input, &mut produced);
        for item in produced.drain(..) {
            match item {
                StreamOutput::Started(id, at) => {
                    send_cooperative(&stream.notices, FrontendNotice::Started(id, at), cancelled)?;
                    bus.publish(PipelineEvent::SpeechStarted {
                        id,
                        at_ms: at.millis(),
                    });
                }
                StreamOutput::Closed(id, end, cut) => {
                    stream.segment_counts.record(cut);
                    send_cooperative(
                        &stream.notices,
                        FrontendNotice::SegmentClosed(id, end),
                        cancelled,
                    )?;
                }
                StreamOutput::Message(message) => send_cooperative(&output, message, cancelled)?,
            }
        }
    }
    Ok(())
}

/// Chinese and mixed lines are translated; English and other languages show as heard.
fn translates(transcript: &Transcript) -> bool {
    transcript.class.is_translated()
}

fn send_cooperative<T>(sender: &Sender<T>, mut message: T, cancelled: &AtomicBool) -> Result<()> {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        match sender.send_timeout(message, POLL) {
            Ok(()) => return Ok(()),
            Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => message = returned,
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                return Err(Error::Stopped)
            }
        }
    }
}

struct Scheduler {
    config: Config,
    joiner: Joiner,
    queue: TranslationQueue,
    ready: VecDeque<Transcript>,
    bus: EventBus,
    observed: BTreeSet<UtteranceId>,
    translating: BTreeSet<UtteranceId>,
    in_flight: bool,
    started_through: Option<UtteranceId>,
    handled_through: Option<UtteranceId>,
    closed_through: Option<UtteranceId>,
    metrics: Metrics,
    latest_stats: Arc<Mutex<PipelineStats>>,
    asr_failed: bool,
}

impl Scheduler {
    fn publish(&mut self, event: PipelineEvent) {
        self.metrics.record(&event);
        self.bus.publish(event);
    }

    fn partial(&mut self, id: UtteranceId, text: String, class: TextClass, end: StreamTime) {
        let end_ms = end.millis();
        self.publish(PipelineEvent::AsrPartial {
            id,
            text,
            class,
            end_ms,
        });
    }

    fn refresh_stats(&mut self, live: StreamTime) {
        let waiting = self
            .queue
            .len()
            .saturating_add(self.ready.len())
            .min(u32::MAX as usize) as u32;
        let snapshot = self
            .metrics
            .snapshot(live, waiting, self.joiner.held_count());
        if let Ok(mut latest) = self.latest_stats.lock() {
            *latest = snapshot;
        }
    }

    fn joined(&mut self, result: JoinResult) {
        for event in result.events {
            if let PipelineEvent::Joined { absorbed, .. } = &event {
                for id in absorbed {
                    self.observed.remove(id);
                    self.translating.remove(id);
                }
            }
            self.publish(event);
        }
        self.ready
            .extend(result.ready.into_iter().filter(translates));
    }

    fn notice(&mut self, notice: FrontendNotice) {
        match notice {
            FrontendNotice::Started(id, at) => {
                self.started_through =
                    Some(self.started_through.map_or(id, |previous| previous.max(id)));
                if self.handled_through.is_none_or(|handled| id > handled) {
                    self.observed.insert(id);
                }
                let result = self.joiner.speech_started(id, at);
                self.joined(result);
            }
            FrontendNotice::Discontinuity(at) => {
                self.queue.discontinuity(at);
                let result = self.joiner.discontinuity_at(at);
                self.joined(result);
            }
            FrontendNotice::Progress(_) => {}
            FrontendNotice::SegmentClosed(id, end) => {
                self.closed_through =
                    Some(self.closed_through.map_or(id, |previous| previous.max(id)));
                self.metrics.segment_closed(id, end);
            }
        }
    }

    fn transcript(&mut self, message: AsrMessage) {
        if let AsrMessage::Partial {
            id,
            text,
            class,
            end,
        } = message
        {
            // No partial after the id's AsrFinal or Dropped.
            if self.handled_through.is_none_or(|handled| id > handled) {
                self.partial(id, text, class, end);
            }
            return;
        }
        let id = message.id();
        self.handled_through = Some(self.handled_through.map_or(id, |previous| previous.max(id)));
        if self.asr_failed && !matches!(&message, AsrMessage::Failed(_, _)) {
            self.asr_failed = false;
            self.publish(PipelineEvent::EngineStatus {
                engine: EngineKind::Asr,
                state: EngineState::Ready,
                message: None,
            });
        }
        match message {
            AsrMessage::Transcript(transcript) => {
                self.observed.insert(transcript.id);
                self.publish(PipelineEvent::AsrFinal {
                    id: transcript.id,
                    text: transcript.text.clone(),
                    class: transcript.class,
                    lang: transcript.lang_tag.clone(),
                    start_ms: transcript.timing.start.millis(),
                    end_ms: transcript.timing.end.millis(),
                    asr_ms: transcript.timing.asr_ms,
                    cut: transcript.cut,
                    logprob: transcript.logprob,
                });
                if translates(&transcript) {
                    self.translating.insert(transcript.id);
                } else {
                    self.observed.remove(&transcript.id);
                }
                let result = self.joiner.push(transcript);
                self.joined(result);
            }
            AsrMessage::Dropped(id, reason) => {
                self.observed.remove(&id);
                self.publish(PipelineEvent::Dropped { id, reason });
                let result = self.joiner.dropped(id);
                self.joined(result);
            }
            AsrMessage::Partial { .. } => {}
            AsrMessage::Failed(id, message) => {
                // There is no ASR-failure line type in the wire contract; the
                // engine error is explicit and no nonexistent ASR text is sent.
                self.observed.remove(&id);
                self.asr_failed = true;
                self.publish(PipelineEvent::EngineStatus {
                    engine: EngineKind::Asr,
                    state: EngineState::Failed,
                    message: Some(message),
                });
                self.publish(PipelineEvent::Dropped {
                    id,
                    reason: DropReason::Empty,
                });
                let result = self.joiner.dropped(id);
                self.joined(result);
            }
        }
    }

    fn completed(&mut self, event: PipelineEvent) {
        if let PipelineEvent::TranslationFinal { id, .. }
        | PipelineEvent::TranslationFailed { id, .. } = &event
        {
            self.observed.remove(id);
            self.translating.remove(id);
        }
        self.publish(event);
        self.in_flight = false;
    }
}

struct CachedSampler {
    inner: Box<dyn ResourceSampler>,
    at: Option<Instant>,
    last: ResourceSample,
}

impl ResourceSampler for CachedSampler {
    fn sample(&mut self) -> ResourceSample {
        if self
            .at
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(1))
        {
            self.last = self.inner.sample();
            self.at = Some(Instant::now());
        }
        self.last
    }
}

#[allow(clippy::too_many_arguments)]
fn scheduler_loop(
    config: Config,
    translator: Box<dyn Translator>,
    front: Receiver<FrontendNotice>,
    asr: Receiver<AsrMessage>,
    clock: Arc<SessionClock>,
    bus: EventBus,
    cancelled: Arc<AtomicBool>,
    listening: Arc<AtomicBool>,
    latest_stats: Arc<Mutex<PipelineStats>>,
    sampler: Option<Box<dyn ResourceSampler>>,
) -> Result<()> {
    // This clock advances only after ordered frontend notices are consumed.
    // Fast replay must not expire a hold using future frames whose speech start
    // notice is still in the channel. Wall time continues between notices.
    let join_clock = Arc::new(ManualClock::default());
    let mut frontier = StreamTime::ZERO;
    let mut anchor = Instant::now();
    let mut metrics = Metrics::new();
    metrics.set_sampler(sampler.map(|inner| {
        Box::new(CachedSampler {
            inner,
            at: None,
            last: ResourceSample::default(),
        }) as Box<dyn ResourceSampler>
    }));
    let mut scheduler = Scheduler {
        joiner: Joiner::new(config.join.clone(), join_clock.clone()),
        queue: TranslationQueue::new(config.translate.clone()),
        config: config.clone(),
        ready: VecDeque::new(),
        bus: bus.clone(),
        observed: BTreeSet::new(),
        translating: BTreeSet::new(),
        in_flight: false,
        started_through: None,
        handled_through: None,
        closed_through: None,
        metrics,
        latest_stats,
        asr_failed: false,
    };
    let mut worker = TranslatorWorker::start(
        translator,
        config.translate,
        clock.clone(),
        bus,
        cancelled.clone(),
    )?;
    let mut front_done = false;
    let mut asr_done = false;
    let mut holds_finished = false;
    let mut pending_asr = None;
    let mut last_stats = Instant::now();
    let mut was_listening = listening.load(Ordering::Acquire);
    scheduler.refresh_stats(clock.now());
    while !cancelled.load(Ordering::Acquire) {
        let mut progressed = false;
        for _ in 0..CONTROL_CAPACITY * 2 {
            match front.try_recv() {
                Ok(notice) => {
                    progressed = true;
                    if let FrontendNotice::Progress(at) = notice {
                        frontier = frontier.max(at);
                        anchor = Instant::now();
                        join_clock.set(frontier.max(join_clock.now()));
                    }
                    scheduler.notice(notice);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    front_done = true;
                    break;
                }
            }
        }
        for event in worker.completed.try_iter() {
            progressed = true;
            scheduler.completed(event);
        }
        while scheduler.queue.len() < TranslationQueue::CAPACITY {
            let Some(transcript) = scheduler.ready.pop_front() else {
                break;
            };
            scheduler.queue.push(transcript, clock.now().millis())?;
            progressed = true;
        }
        // A Joiner push can release two transcripts. Keep that bounded staging
        // space available and stop reading ASR when translation is saturated.
        if !asr_done
            && scheduler.ready.is_empty()
            && scheduler.queue.len() < TranslationQueue::CAPACITY - 2
        {
            let message = if let Some(message) = pending_asr.take() {
                Some(message)
            } else {
                match asr.try_recv() {
                    Ok(message) => Some(message),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => {
                        asr_done = true;
                        progressed = true;
                        None
                    }
                }
            };
            if let Some(message) = message {
                // The control and transcript channels have independent senders.
                // A finite control burst must still consume this segment's own
                // start notice before routing its transcript or timing its hold.
                // A partial only needs its start notice: the clause is still open.
                let started = scheduler
                    .started_through
                    .is_some_and(|id| id >= message.id());
                let closed = scheduler
                    .closed_through
                    .is_some_and(|id| id >= message.id());
                let is_partial = matches!(message, AsrMessage::Partial { .. });
                if front_done || (started && (closed || is_partial)) {
                    scheduler.transcript(message);
                    progressed = true;
                } else {
                    pending_asr = Some(message);
                }
            }
        }
        if front.is_empty() {
            let wall_progress = StreamTime::from_seconds(anchor.elapsed().as_secs_f64());
            let at = StreamTime(frontier.samples().saturating_add(wall_progress.samples()));
            join_clock.set(at.max(join_clock.now()));
            let result = scheduler.joiner.tick();
            progressed |= !result.ready.is_empty() || !result.events.is_empty();
            scheduler.joined(result);
        }
        // Frontend EOF alone is too early: a timely successor may still be in
        // the bounded ASR channel or an active native decode call.
        if asr_done && !holds_finished {
            let result = scheduler.joiner.finish();
            scheduler.joined(result);
            holds_finished = true;
            progressed = true;
        }
        if !scheduler.in_flight && !scheduler.queue.is_empty() {
            let batch = scheduler.queue.take(clock.now());
            for event in batch.events {
                match &event {
                    PipelineEvent::Skipped { id, .. } => {
                        scheduler.observed.remove(id);
                        scheduler.translating.remove(id);
                    }
                    PipelineEvent::Joined { absorbed, .. } => {
                        for id in absorbed {
                            scheduler.observed.remove(id);
                            scheduler.translating.remove(id);
                        }
                    }
                    _ => {}
                }
                scheduler.publish(event);
            }
            if let Some(item) = batch.item {
                send_cooperative(&worker.input, item, &cancelled)?;
                scheduler.in_flight = true;
                progressed = true;
            }
        }
        if asr_done
            && front_done
            && scheduler.ready.is_empty()
            && scheduler.queue.is_empty()
            && !scheduler.in_flight
        {
            break;
        }
        scheduler.refresh_stats(clock.now());
        let is_listening = listening.load(Ordering::Acquire);
        if is_listening != was_listening {
            was_listening = is_listening;
            last_stats = Instant::now();
        }
        if is_listening && last_stats.elapsed() >= Duration::from_secs(1) {
            // Frontend clears listening under this lock before acknowledging
            // pause. A timer cannot publish a stale Stats after that boundary.
            if let Ok(latest) = scheduler.latest_stats.lock() {
                if listening.load(Ordering::Acquire) && !cancelled.load(Ordering::Acquire) {
                    scheduler.bus.publish(PipelineEvent::Stats(latest.clone()));
                }
            }
            last_stats = Instant::now();
        }
        // Available messages wake the stage without an artificial frame/ASR
        // throttle. Idle polling remains bounded and does not spin a CPU core.
        if !progressed {
            thread::sleep(POLL);
        }
    }
    worker.stop()?;
    // Wait for producers to close before publishing shutdown terminals. This
    // prevents a racing SpeechStarted or delta from appearing after a terminal.
    while !front_done || !asr_done {
        if !front_done {
            match front.recv_timeout(POLL) {
                Ok(FrontendNotice::Started(id, _)) => {
                    if scheduler.handled_through.is_none_or(|handled| id > handled) {
                        scheduler.observed.insert(id);
                    }
                }
                Ok(FrontendNotice::SegmentClosed(id, end)) => {
                    scheduler.metrics.segment_closed(id, end);
                }
                Ok(_) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => front_done = true,
            }
        }
        if !asr_done {
            match asr.recv_timeout(POLL) {
                Ok(_) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => asr_done = true,
            }
        }
    }
    for event in worker.completed.try_iter() {
        scheduler.completed(event);
    }
    let unfinished: Vec<_> = scheduler.observed.iter().copied().collect();
    for id in unfinished {
        scheduler.publish(if scheduler.translating.contains(&id) {
            PipelineEvent::TranslationFailed {
                id,
                reason: FailReason::Error,
                message: "Listening stopped before this line finished".into(),
            }
        } else {
            PipelineEvent::Dropped {
                id,
                reason: DropReason::Empty,
            }
        });
    }
    scheduler.ready.clear();
    scheduler.queue = TranslationQueue::new(scheduler.config.translate.clone());
    let _ = scheduler.joiner.finish();
    scheduler.in_flight = false;
    scheduler.refresh_stats(clock.now());
    Ok(())
}
