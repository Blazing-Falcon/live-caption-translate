//! Bounded, independently scheduled audio/ASR/translation workers.
//!
//! Audio adapters deliver prepared 16 kHz frames. Engines remain owned by one
//! worker each; source cancellation and whole-pipeline shutdown are separate.
//!
//! Threads: `lt-frontend` turns capture frames into recognizer input, `lt-asr`
//! owns the recognizer, and `lt-scheduler` owns the translation queue, the
//! translator worker and the stats.

mod asr;
mod frontend;
mod handle;
mod scheduler;

pub use handle::{PipelineHandle, SegmentCounts};

use crate::{
    bus::EventBus,
    config::Config,
    engines::{StreamingAsr, Translator},
    error::{Error, Result},
    events::{ListeningStateKind, PipelineEvent},
    metrics::ResourceSampler,
    source::{AudioSource, SessionClock},
    types::{PipelineStats, StreamTime, UtteranceId},
};
use crossbeam_channel::{bounded, Sender};
use handle::SegmentCounters;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

const POLL: Duration = Duration::from_millis(5);
const CONTROL_CAPACITY: usize = 16;
const ASR_CAPACITY: usize = 4;

enum Command {
    Pause(Sender<Result<()>>),
    Resume(Sender<Result<()>>),
    Reopen(Box<dyn AudioSource>, Sender<Result<()>>),
}

#[derive(Clone, Copy)]
enum FrontendNotice {
    Started(UtteranceId),
    Discontinuity(StreamTime),
    SegmentClosed(UtteranceId, StreamTime),
}

pub struct Pipeline;

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
        let asr_notices = asr::AsrNotices {
            notices: front_tx.clone(),
            segment_counts: segment_counts.clone(),
        };
        let asr_thread = thread::Builder::new()
            .name("lt-asr".into())
            .spawn(move || {
                asr::asr_loop(
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
                    scheduler::scheduler_loop(
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
                    frontend::frontend_loop(
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

struct Finished(Arc<AtomicBool>);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
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
