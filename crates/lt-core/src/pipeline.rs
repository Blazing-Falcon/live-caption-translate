//! Bounded, independently scheduled audio/ASR/translation workers.
//!
//! Audio adapters deliver prepared 16 kHz frames. Engines remain owned by one
//! worker each; source cancellation and whole-pipeline shutdown are separate.

use crate::{
    bus::{EventBus, EventReceiver},
    clock::{Clock, ManualClock},
    config::{Config, LatencyConfig, RoutingConfig},
    drafts::{
        DraftOutcome, DraftResult, DraftState, DraftWorker, PublishedDraft, WORKER_FAILED,
        WORKER_READY,
    },
    engines::{SegmentAsr, Translator, Vad},
    error::{Error, Result},
    events::{DropReason, EngineKind, EngineState, FailReason, ListeningStateKind, PipelineEvent},
    join::{JoinResult, Joiner},
    metrics::{canonical_language, Metrics, ResourceSample, ResourceSampler},
    queue::{TranslationQueue, TranslatorWorker},
    recognizer::{AsrMessage, CommitCut, Output, Recognizer, RecognizerInput},
    segment::{SegmentBuilder, SegmentUpdate},
    source::{AudioFrame, AudioProducer, AudioReceiver, AudioSource, SessionClock, SourceEvents},
    stepdown::{ModeState, StepDown},
    types::{
        CutReason, EffectiveMode, ModeReason, PipelineStats, StreamTime, TextClass, Transcript,
        UtteranceId,
    },
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
/// Commit requests are rare and tiny; a full channel simply skips one.
const COMMIT_CAPACITY: usize = 4;
/// The open clause's audio goes to the recognizer at least this often.
const AUDIO_FORWARD_SAMPLES: u64 = 1_600;

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
    pub soft_cut: u64,
    pub hard_cut: u64,
    pub discontinuity: u64,
    pub end: u64,
    pub commit: u64,
}

impl SegmentCounts {
    pub fn total(self) -> u64 {
        self.pause + self.soft_cut + self.hard_cut + self.discontinuity + self.end + self.commit
    }
}

#[derive(Default)]
struct SegmentCounters([AtomicU64; 6]);

impl SegmentCounters {
    fn record(&self, reason: CutReason) {
        let index = match reason {
            CutReason::Pause => 0,
            CutReason::SoftCut => 1,
            CutReason::HardCut => 2,
            CutReason::Discontinuity => 3,
            CutReason::End => 4,
            CutReason::Commit => 5,
        };
        self.0[index].fetch_add(1, Ordering::Relaxed);
    }
    fn snapshot(&self) -> SegmentCounts {
        let counts = self.0.each_ref().map(|count| count.load(Ordering::Relaxed));
        SegmentCounts {
            pause: counts[0],
            soft_cut: counts[1],
            hard_cut: counts[2],
            discontinuity: counts[3],
            end: counts[4],
            commit: counts[5],
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
    modes: Sender<EffectiveMode>,
    initial_mode: ModeState,
}

/// Optional parts of a pipeline.
#[derive(Default)]
pub struct PipelineOptions {
    pub sampler: Option<Box<dyn ResourceSampler>>,
    /// The draft translator (Continuous mode). Without it the pipeline runs Light at most.
    pub draft: Option<Box<dyn Translator>>,
    /// Physical cores, for `latency.mode = "auto"`. Unknown counts half the logical CPUs.
    pub physical_cores: Option<usize>,
}

/// How `latency.mode` resolves for this engine set.
pub fn resolve_mode(
    config: &LatencyConfig,
    windowed_asr: bool,
    draft_available: bool,
    physical_cores: usize,
) -> ModeState {
    let (mode, reason) = if !windowed_asr {
        (EffectiveMode::Off, None)
    } else {
        match config.mode.as_str() {
            "off" => (EffectiveMode::Off, Some(ModeReason::User)),
            "light" => (EffectiveMode::Light, Some(ModeReason::User)),
            "continuous" if draft_available => (EffectiveMode::Continuous, Some(ModeReason::User)),
            "continuous" => (EffectiveMode::Light, Some(ModeReason::DraftUnavailable)),
            _ if draft_available && physical_cores >= config.auto_min_cores as usize => {
                (EffectiveMode::Continuous, Some(ModeReason::Auto))
            }
            _ => (EffectiveMode::Light, Some(ModeReason::Auto)),
        }
    };
    ModeState { mode, reason }
}

impl Pipeline {
    pub fn start(
        config: Config,
        source: Box<dyn AudioSource>,
        vad: Box<dyn Vad>,
        asr: Box<dyn SegmentAsr>,
        translator: Box<dyn Translator>,
    ) -> Result<PipelineHandle> {
        Self::start_with_bus(config, source, vad, asr, translator, EventBus::default())
    }

    /// Subscribe before calling this method when replay can finish immediately.
    pub fn start_with_bus(
        config: Config,
        source: Box<dyn AudioSource>,
        vad: Box<dyn Vad>,
        asr: Box<dyn SegmentAsr>,
        translator: Box<dyn Translator>,
        bus: EventBus,
    ) -> Result<PipelineHandle> {
        Self::start_with_bus_and_sampler(config, source, vad, asr, translator, bus, None)
    }

    pub fn start_with_bus_and_sampler(
        config: Config,
        source: Box<dyn AudioSource>,
        vad: Box<dyn Vad>,
        asr: Box<dyn SegmentAsr>,
        translator: Box<dyn Translator>,
        bus: EventBus,
        sampler: Option<Box<dyn ResourceSampler>>,
    ) -> Result<PipelineHandle> {
        Self::start_with_options(
            config,
            source,
            vad,
            asr,
            translator,
            bus,
            PipelineOptions {
                sampler,
                ..PipelineOptions::default()
            },
        )
    }

    pub fn start_with_options(
        mut config: Config,
        source: Box<dyn AudioSource>,
        vad: Box<dyn Vad>,
        mut asr: Box<dyn SegmentAsr>,
        translator: Box<dyn Translator>,
        bus: EventBus,
        options: PipelineOptions,
    ) -> Result<PipelineHandle> {
        let PipelineOptions {
            sampler,
            draft,
            physical_cores,
        } = options;
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
        let (segment_tx, segment_rx) = bounded(ASR_CAPACITY);
        let (front_tx, front_rx) = bounded(CONTROL_CAPACITY);
        let (asr_tx, asr_rx) = bounded(ASR_CAPACITY);
        let (commit_tx, commit_rx) = bounded(COMMIT_CAPACITY);
        let (mode_tx, mode_rx) = bounded(CONTROL_CAPACITY);
        let (ready_tx, ready_rx) = bounded(1);
        // An engine without a windowed decode cannot run the recognizer: that case runs in Off mode.
        let cores = physical_cores
            .unwrap_or_else(|| thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1)));
        let initial_mode = resolve_mode(
            &config.latency,
            asr.window().is_some(),
            draft.is_some(),
            cores,
        );
        let recognizer_active = initial_mode.mode != EffectiveMode::Off;
        let mut handle = PipelineHandle {
            commands: command_tx,
            cancelled: cancelled.clone(),
            finished: finished.clone(),
            bus: bus.clone(),
            threads: Vec::with_capacity(3),
            latest_stats: latest_stats.clone(),
            segment_counts: segment_counts.clone(),
            modes: mode_tx,
            initial_mode,
        };
        if let Ok(mut stats) = latest_stats.lock() {
            stats.mode = initial_mode.mode;
            stats.mode_reason = initial_mode.reason;
        }
        bus.publish(PipelineEvent::ListeningState {
            state: ListeningStateKind::Starting,
        });

        let asr_cancel = cancelled.clone();
        let asr_clock = clock.clone();
        let asr_bus = bus.clone();
        let asr_config = config.clone();
        let asr_thread = thread::Builder::new()
            .name("lt-asr".into())
            .spawn(move || {
                asr_loop(
                    asr, asr_config, segment_rx, commit_tx, asr_tx, asr_clock, asr_bus, asr_cancel,
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
                        draft,
                        initial_mode,
                        mode_rx,
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
                        vad,
                        config,
                        command_rx,
                        segment_tx,
                        commit_rx,
                        recognizer_active,
                        front_tx,
                        clock,
                        front_bus,
                        front_cancel,
                        ready_tx,
                        listening,
                        latest_stats,
                        segment_counts,
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

    /// The mode the pipeline started in and why.
    pub fn initial_mode(&self) -> ModeState {
        self.initial_mode
    }

    /// Switch between Continuous and Light while running. Off needs a restart: the recognizer
    /// is not fed audio in Off mode.
    pub fn set_mode(&self, mode: EffectiveMode) -> Result<()> {
        if mode == EffectiveMode::Off || self.initial_mode.mode == EffectiveMode::Off {
            return Err(Error::Engine(
                "Switching to or from Off restarts the pipeline".into(),
            ));
        }
        self.modes
            .try_send(mode)
            .map_err(|_| Error::Engine("Mode change not accepted".into()))
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
    vad: Box<dyn Vad>,
    builder: SegmentBuilder,
    input: Option<AudioReceiver>,
    source_cancel: Arc<AtomicBool>,
    inputs: VecDeque<RecognizerInput>,
    notices: VecDeque<FrontendNotice>,
    clock: Arc<SessionClock>,
    bus: EventBus,
    /// Forward the open clause's audio to the recognizer (Continuous and Light); Off sends
    /// only `Closed`, exactly like the segment path.
    recognizer: bool,
    /// The clause whose audio the recognizer holds, and the stream sample sent so far.
    forwarded: Option<(UtteranceId, u64)>,
    ending: bool,
    started_once: bool,
    listening: Arc<AtomicBool>,
    latest_stats: Arc<Mutex<PipelineStats>>,
    segment_counts: Arc<SegmentCounters>,
}

impl Frontend {
    fn start_source(&mut self) -> Result<()> {
        let at = if self.started_once {
            self.clock.now().max(self.builder.current_time())
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
        let update = self.builder.discontinuity(at);
        self.notices.push_back(FrontendNotice::Discontinuity(at));
        self.stage(update, false);
        self.vad.reset();
    }

    /// Queue an input for the recognizer, merging consecutive audio of the same clause.
    fn push_input(&mut self, input: RecognizerInput) {
        if let (
            RecognizerInput::Audio { id, samples },
            Some(RecognizerInput::Audio {
                id: last_id,
                samples: last,
            }),
        ) = (&input, self.inputs.back_mut())
        {
            if id == last_id {
                last.extend_from_slice(samples);
                return;
            }
        }
        self.inputs.push_back(input);
    }

    /// Send the open clause's audio that the recognizer does not hold yet.
    fn sync_audio(&mut self, force: bool) {
        let Some((id, sent)) = self.forwarded else {
            return;
        };
        let current = self.builder.current_time().0;
        if !force && current.saturating_sub(sent) < AUDIO_FORWARD_SAMPLES {
            return;
        }
        if let Some((open, samples)) = self.builder.open_audio(sent) {
            if open == id && !samples.is_empty() {
                self.forwarded = Some((id, current));
                self.push_input(RecognizerInput::Audio { id, samples });
            }
        }
    }

    /// A clause closed: the recognizer receives the audio it is missing, then the segment.
    fn close_forwarding(&mut self, segment: &crate::types::Segment) {
        if let Some((id, sent)) = self.forwarded.filter(|(id, _)| *id == segment.id) {
            if sent < segment.end.0 {
                let from = sent.max(segment.start.0);
                let offset = (from - segment.start.0) as usize;
                if let Some(rest) = segment.samples.get(offset..).filter(|r| !r.is_empty()) {
                    self.push_input(RecognizerInput::Audio {
                        id,
                        samples: rest.to_vec(),
                    });
                }
            }
            self.forwarded = None;
        }
    }

    fn stage(&mut self, update: SegmentUpdate, after_commit: bool) {
        let started = update.started;
        self.notices.extend(
            started
                .iter()
                .map(|(id, at)| FrontendNotice::Started(*id, *at)),
        );
        for segment in update.segments {
            self.segment_counts.record(segment.cut_reason);
            self.notices
                .push_back(FrontendNotice::SegmentClosed(segment.id, segment.end));
            if self.recognizer {
                self.close_forwarding(&segment);
            }
            self.push_input(RecognizerInput::Closed(segment));
        }
        if self.recognizer {
            for (id, _) in started {
                // Only the clause that is still open can receive audio.
                if let Some((open, start)) = self.builder.open_clause().filter(|(o, _)| *o == id) {
                    self.push_input(RecognizerInput::Open {
                        id: open,
                        start,
                        after_commit,
                    });
                    self.forwarded = Some((open, start.0));
                    self.sync_audio(true);
                }
            }
        }
    }

    /// The recognizer committed a clause: cut the open clause where it asked.
    fn commit(&mut self, cut: CommitCut) {
        let update = self.builder.commit_cut(cut.id, cut.at);
        if !update.segments.is_empty() {
            self.stage(update, true);
        }
    }

    fn command(&mut self, command: Command) {
        let (reply, result) = match command {
            Command::Pause(reply) => {
                if self.input.is_some() {
                    self.stop_source();
                    self.cut(self.builder.current_time());
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
                self.cut(self.builder.current_time());
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

    fn frame(&mut self, mut frame: AudioFrame) {
        if frame.t0 < self.builder.current_time() {
            return;
        }
        if frame.t0 != self.builder.current_time() || frame.flags.discontinuity {
            self.cut(frame.t0);
            frame.flags.discontinuity = false;
        }
        let probability = self.vad.speech_prob(&frame.samples);
        let update = self.builder.push(&frame.samples, probability, frame.flags);
        self.stage(update, false);
        if self.recognizer {
            self.sync_audio(false);
        }
        let at = self.builder.current_time();
        self.clock.advance_to(at);
        self.notices.push_back(FrontendNotice::Progress(at));
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
    vad: Box<dyn Vad>,
    config: Config,
    commands: Receiver<Command>,
    segments: Sender<RecognizerInput>,
    commits: Receiver<CommitCut>,
    recognizer: bool,
    notices: Sender<FrontendNotice>,
    clock: Arc<SessionClock>,
    bus: EventBus,
    cancelled: Arc<AtomicBool>,
    ready: Sender<Result<()>>,
    listening: Arc<AtomicBool>,
    latest_stats: Arc<Mutex<PipelineStats>>,
    segment_counts: Arc<SegmentCounters>,
) -> Result<()> {
    let mut frontend = Frontend {
        source,
        vad,
        builder: SegmentBuilder::new(config.vad),
        input: None,
        source_cancel: Arc::new(AtomicBool::new(false)),
        inputs: VecDeque::new(),
        notices: VecDeque::new(),
        clock,
        bus,
        recognizer,
        forwarded: None,
        ending: false,
        started_once: false,
        listening,
        latest_stats,
        segment_counts,
    };
    if let Err(error) = frontend.start_source() {
        let _ = ready.try_send(Err(Error::Engine(error.to_string())));
        cancelled.store(true, Ordering::Release);
        return Err(error);
    }
    frontend.bus.publish(PipelineEvent::EngineStatus {
        engine: EngineKind::Vad,
        state: EngineState::Ready,
        message: None,
    });
    let _ = ready.try_send(Ok(()));
    while !cancelled.load(Ordering::Acquire) {
        // Commands stay responsive even if segments are backpressured by ASR.
        for command in commands.try_iter().take(CONTROL_CAPACITY) {
            frontend.command(command);
        }
        for cut in commits.try_iter().take(COMMIT_CAPACITY) {
            frontend.commit(cut);
        }
        while let Some(notice) = frontend.notices.pop_front() {
            match notices.try_send(notice) {
                Ok(()) => {
                    if let FrontendNotice::Started(id, at) = notice {
                        frontend.bus.publish(PipelineEvent::SpeechStarted {
                            id,
                            at_ms: at.millis(),
                        });
                    }
                }
                Err(TrySendError::Full(notice)) => {
                    frontend.notices.push_front(notice);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => return Err(Error::Stopped),
            }
        }
        if frontend.notices.is_empty() {
            while let Some(input) = frontend.inputs.pop_front() {
                match segments.try_send(input) {
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
                    if let FrontendNotice::Started(id, at) = notice {
                        frontend.bus.publish(PipelineEvent::SpeechStarted { id, at_ms: at.millis() });
                    }
                },
                recv(commands) -> command => match command { Ok(command) => frontend.command(command), Err(_) => break },
                default(POLL) => {}
            }
            continue;
        }
        if let Some(input) = frontend.inputs.front().cloned() {
            crossbeam_channel::select! {
                send(segments, input) -> result => {
                    result.map_err(|_| Error::Stopped)?;
                    frontend.inputs.pop_front();
                },
                recv(commands) -> command => match command { Ok(command) => frontend.command(command), Err(_) => break },
                recv(commits) -> cut => if let Ok(cut) = cut { frontend.commit(cut) },
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
            recv(commits) -> cut => if let Ok(cut) = cut { frontend.commit(cut) },
            recv(input) -> frame => match frame {
                Ok(frame) => frontend.frame(frame),
                Err(_) => {
                    frontend.stop_source();
                    let update = frontend.builder.finish();
                    frontend.stage(update, false);
                    frontend.ending = true;
                }
            },
            default(POLL) => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn asr_loop(
    mut asr: Box<dyn SegmentAsr>,
    config: Config,
    inputs: Receiver<RecognizerInput>,
    commits: Sender<CommitCut>,
    output: Sender<AsrMessage>,
    clock: Arc<SessionClock>,
    bus: EventBus,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    bus.publish(PipelineEvent::EngineStatus {
        engine: EngineKind::Asr,
        state: EngineState::Ready,
        message: None,
    });
    let mut recognizer = Recognizer::new(&config);
    let mut produced = Vec::new();
    while !cancelled.load(Ordering::Acquire) {
        let input = match inputs.recv_timeout(POLL) {
            Ok(input) => input,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        let id = match &input {
            RecognizerInput::Open { id, .. } | RecognizerInput::Audio { id, .. } => *id,
            RecognizerInput::Closed(segment) => segment.id,
        };
        let span = tracing::info_span!("asr", id = id.0);
        let _utterance = span.enter();
        produced.clear();
        recognizer.handle(asr.as_mut(), input, clock.now(), &mut produced);
        for item in produced.drain(..) {
            match item {
                Output::Message(message) => send_cooperative(&output, message, &cancelled)?,
                Output::Cut(cut) => {
                    // A full channel means the frontend is behind; the next decode asks again.
                    let _ = commits.try_send(cut);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn normalized_language(tag: Option<&str>) -> Option<String> {
    let tag = tag?.trim();
    let tag = tag
        .strip_prefix("<|")
        .and_then(|tag| tag.strip_suffix("|>"))
        .unwrap_or(tag)
        .trim();
    let tag = tag.to_ascii_lowercase();
    let primary = tag.split(['-', '_']).next().unwrap_or(&tag);
    let language = match primary {
        "ja" | "jpn" | "japanese" => "ja",
        "ko" | "kor" | "korean" => "ko",
        "zh" | "zho" | "chi" | "chinese" | "mandarin" => "zh",
        "en" | "eng" | "english" => "en",
        _ => &tag,
    };
    (!language.is_empty()).then(|| language.to_owned())
}

pub(crate) fn inferred_other_language(text: &str) -> Option<String> {
    let kana = text
        .chars()
        .filter(|character| matches!(character, '\u{3040}'..='\u{30ff}'))
        .count();
    let hangul = text
        .chars()
        .filter(|character| matches!(character, '\u{ac00}'..='\u{d7af}'))
        .count();
    if hangul > kana {
        Some("ko".into())
    } else if kana > 0 {
        Some("ja".into())
    } else {
        None
    }
}

fn translates(transcript: &Transcript, config: &RoutingConfig) -> bool {
    match transcript.class {
        TextClass::Chinese | TextClass::Mixed => true,
        TextClass::English => false,
        TextClass::Other => transcript.lang_tag.as_ref().is_some_and(|lang| {
            config
                .translate_other
                .iter()
                .any(|enabled| canonical_language(enabled) == *lang)
        }),
    }
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
    /// Speech end of the final translation now in flight (the step-down's lag input).
    final_end: Option<StreamTime>,
    started_through: Option<UtteranceId>,
    handled_through: Option<UtteranceId>,
    closed_through: Option<UtteranceId>,
    metrics: Metrics,
    latest_stats: Arc<Mutex<PipelineStats>>,
    asr_failed: bool,
    clock: Arc<SessionClock>,
    mode: ModeState,
    stepdown: StepDown,
    drafts: DraftState,
    /// The draft worker's warm-up result; drafts run only while it is ready.
    draft_ready: bool,
    /// A draft request is out (counts as the single request in flight).
    draft_in_flight: bool,
}

impl Scheduler {
    fn publish(&mut self, event: PipelineEvent) {
        self.metrics.record_at(&event, self.clock.now().millis());
        self.bus.publish(event);
    }

    /// Apply a new effective mode: leaving Continuous drops the waiting draft.
    fn apply_mode(&mut self, state: ModeState) {
        if state == self.mode {
            return;
        }
        tracing::info!(mode = ?state.mode, reason = ?state.reason, "caption mode changed");
        self.mode = state;
        if state.mode != EffectiveMode::Continuous {
            self.drafts.clear_slot();
        }
    }

    fn drafts_on(&self) -> bool {
        self.mode.mode == EffectiveMode::Continuous && self.draft_ready
    }

    fn partial(&mut self, id: UtteranceId, text: String, class: TextClass, end: StreamTime) {
        let end_ms = end.millis();
        if self.drafts_on() && matches!(class, TextClass::Chinese | TextClass::Mixed) {
            self.drafts.partial(id, &text, "zh", end_ms);
        }
        self.publish(PipelineEvent::AsrPartial {
            id,
            text,
            class,
            end_ms,
        });
    }

    fn draft_completed(&mut self, outcome: DraftOutcome) {
        self.draft_in_flight = false;
        self.in_flight = false;
        match &outcome.result {
            DraftResult::Failed(_) => {
                if let Some(state) = self.stepdown.draft_failed() {
                    self.apply_mode(state);
                }
                self.metrics.draft_failed();
            }
            DraftResult::Text(_) => self.stepdown.draft_succeeded(),
            DraftResult::Rejected => {}
        }
        if let Some(PublishedDraft {
            id,
            rev,
            text,
            end_ms,
        }) = self.drafts.completed(outcome)
        {
            self.publish(PipelineEvent::TranslationDraft {
                id,
                rev,
                text,
                end_ms,
            });
        }
    }

    fn refresh_stats(&mut self, live: StreamTime) {
        let waiting = self
            .queue
            .len()
            .saturating_add(self.ready.len())
            .min(u32::MAX as usize) as u32;
        let mut snapshot = self
            .metrics
            .snapshot(live, waiting, self.joiner.held_count());
        snapshot.mode = self.mode.mode;
        snapshot.mode_reason = self.mode.reason;
        snapshot.drafts_total = self.drafts.published_total();
        snapshot.drafts_failed = self.drafts.failed_total();
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
                    self.drafts.terminal(*id);
                }
            }
            self.publish(event);
        }
        self.ready.extend(
            result
                .ready
                .into_iter()
                .filter(|transcript| translates(transcript, &self.config.routing)),
        );
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
                });
                self.drafts.final_arrived(transcript.id);
                if translates(&transcript, &self.config.routing) {
                    self.translating.insert(transcript.id);
                } else {
                    self.observed.remove(&transcript.id);
                }
                let result = self.joiner.push(transcript);
                self.joined(result);
            }
            AsrMessage::Dropped(id, reason) => {
                self.observed.remove(&id);
                self.drafts.terminal(id);
                self.publish(PipelineEvent::Dropped { id, reason });
                let result = self.joiner.dropped(id);
                self.joined(result);
            }
            AsrMessage::Partial { .. } => {}
            AsrMessage::Failed(id, message) => {
                // There is no ASR-failure line type in the wire contract; the
                // engine error is explicit and no nonexistent ASR text is sent.
                self.observed.remove(&id);
                self.drafts.terminal(id);
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
            self.drafts.terminal(*id);
        }
        self.publish(event);
        self.in_flight = false;
        self.final_end = None;
    }

    /// How far the translation backlog trails the stream: the oldest final that is queued,
    /// ready or in flight. Phrases the joiner holds do not count; waiting for the next phrase
    /// is not the translator falling behind (the step-down must not react to it).
    fn translation_lag_ms(&self, live: StreamTime) -> u64 {
        [
            self.final_end,
            self.queue.oldest_end(),
            self.ready.front().map(|t| t.timing.end),
        ]
        .into_iter()
        .flatten()
        .min()
        .map_or(0, |end| live.millis().saturating_sub(end.millis()))
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
    draft: Option<Box<dyn Translator>>,
    initial_mode: ModeState,
    modes: Receiver<EffectiveMode>,
) -> Result<()> {
    // This clock advances only after ordered frontend notices are consumed.
    // Fast replay must not expire a hold using future frames whose speech start
    // notice is still in the channel. Wall time continues between notices.
    let join_clock = Arc::new(ManualClock::default());
    let mut frontier = StreamTime::ZERO;
    let mut anchor = Instant::now();
    let mut metrics = Metrics::with_routing(&config.routing.translate_other);
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
        final_end: None,
        started_through: None,
        handled_through: None,
        closed_through: None,
        metrics,
        latest_stats,
        asr_failed: false,
        clock: clock.clone(),
        mode: initial_mode,
        stepdown: StepDown::new(&config.latency, initial_mode.mode, initial_mode.reason),
        drafts: DraftState::new(&config.latency),
        draft_ready: false,
        draft_in_flight: false,
    };
    let mut draft_worker = match draft {
        Some(translator) if initial_mode.mode != EffectiveMode::Off => Some(DraftWorker::start(
            translator,
            config.latency.clone(),
            bus.clone(),
            cancelled.clone(),
        )?),
        _ => None,
    };
    let session_start = Instant::now();
    let mut last_step = Instant::now();
    let mut draft_known = None;
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
        if let Some(draft_worker) = draft_worker.as_ref() {
            for outcome in draft_worker.completed.try_iter() {
                progressed = true;
                scheduler.draft_completed(outcome);
            }
            let state = draft_worker.state.load(Ordering::Acquire);
            if draft_known != Some(state) {
                draft_known = Some(state);
                scheduler.draft_ready = state == WORKER_READY;
                if state == WORKER_READY || state == WORKER_FAILED {
                    if let Some(next) = scheduler.stepdown.draft_server(state == WORKER_READY) {
                        scheduler.apply_mode(next);
                    }
                }
            }
        }
        for mode in modes.try_iter() {
            if let Some(next) = scheduler
                .stepdown
                .choose(mode, session_start.elapsed().as_secs_f64())
            {
                scheduler.apply_mode(next);
            }
            // Choosing Continuous without a ready draft server falls back to Light.
            if mode == EffectiveMode::Continuous {
                if let Some(next) = scheduler.stepdown.draft_server(scheduler.draft_ready) {
                    scheduler.apply_mode(next);
                }
            }
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
        if !scheduler.in_flight
            && scheduler.queue.is_empty()
            && scheduler.drafts_on()
            && scheduler.drafts.has_job()
        {
            // Finals first (the branch below); a draft only takes an idle translator slot.
            if let (Some(draft_worker), Some(request)) =
                (draft_worker.as_ref(), scheduler.drafts.take_request())
            {
                send_cooperative(&draft_worker.input, request, &cancelled)?;
                scheduler.in_flight = true;
                scheduler.draft_in_flight = true;
                progressed = true;
            }
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
                scheduler.final_end = Some(item.transcript.timing.end);
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
        if is_listening && last_step.elapsed() >= Duration::from_secs(1) {
            last_step = Instant::now();
            let lag_s = scheduler.translation_lag_ms(clock.now()) as f64 / 1_000.0;
            let cpu = scheduler.metrics.system_cpu_pct();
            if let Some(next) =
                scheduler
                    .stepdown
                    .sample(session_start.elapsed().as_secs_f64(), cpu, lag_s)
            {
                scheduler.apply_mode(next);
            }
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
    if let Some(draft_worker) = draft_worker.as_mut() {
        draft_worker.stop()?;
    }
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
    scheduler.drafts.clear_slot();
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
