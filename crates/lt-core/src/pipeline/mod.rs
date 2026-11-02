//! Bounded, independently scheduled audio/ASR/translation workers.
//!
//! `lt-frontend` sends capture frames to the recognizer, `lt-asr` owns the streaming recognizer,
//! `lt-refine` owns the second pass and the filter, and `lt-scheduler` owns the queue, the
//! translator worker and the stats.

mod asr;
mod cancel;
mod frontend;
mod handle;
pub(crate) mod queue;
mod refine;
mod running;
mod scheduler;
mod translator;

pub use handle::{ClauseCounts, PipelineHandle};

use crate::{
    bus::EventBus,
    config::Config,
    engines::{OfflineAsr, StreamingAsr, Translator},
    error::{Error, Result},
    events::{ListeningStateKind, PipelineEvent},
    metrics::ResourceSampler,
    source::{AudioSource, SessionClock},
    types::{PipelineStats, StreamTime, UtteranceId},
};
use cancel::Cancel;
use crossbeam_channel::{bounded, Sender};
use handle::ClauseCounters;
use std::{
    sync::{atomic::AtomicBool, Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

const POLL: Duration = Duration::from_millis(5);
/// Lines waiting for translation before the recognizer is held back.
pub const QUEUE_CAPACITY: usize = 256;
const CONTROL_CAPACITY: usize = 16;
const ASR_CAPACITY: usize = 4;
/// Closed clauses waiting for the second pass. A clause that waits too long skips its decode,
/// so a full queue empties quickly and holds the recognizer only briefly.
const REFINE_CAPACITY: usize = 8;

/// The engines the pipeline owns, one thread each. Without a second pass, closed clauses keep
/// their live text.
pub struct Engines {
    pub asr: Box<dyn StreamingAsr>,
    pub second_pass: Option<Box<dyn OfflineAsr>>,
    pub translator: Box<dyn Translator>,
}

/// State the pipeline threads share besides their channels.
pub(super) struct Shared {
    pub(super) cancelled: Cancel,
    pub(super) clock: Arc<SessionClock>,
    pub(super) listening: AtomicBool,
    pub(super) latest_stats: Mutex<PipelineStats>,
    pub(super) clause_counts: ClauseCounters,
}

enum Command {
    Pause(Sender<Result<()>>),
    Resume(Sender<Result<()>>),
    Reopen(Box<dyn AudioSource>, Sender<Result<()>>),
}

#[derive(Clone, Copy)]
enum FrontendNotice {
    Started(UtteranceId),
    Discontinuity(StreamTime),
    ClauseClosed(UtteranceId, StreamTime),
}

pub struct Pipeline;

impl Pipeline {
    /// Subscribe to `bus` before this call: a short replay can finish before it returns.
    pub fn start(
        mut config: Config,
        source: Box<dyn AudioSource>,
        engines: Engines,
        bus: EventBus,
        sampler: Option<Box<dyn ResourceSampler>>,
    ) -> Result<PipelineHandle> {
        for message in config.validate() {
            tracing::warn!(message, "Pipeline config adjusted");
        }
        let shared = Arc::new(Shared {
            cancelled: Cancel::default(),
            clock: Arc::new(SessionClock::default()),
            listening: AtomicBool::new(false),
            latest_stats: Mutex::new(PipelineStats::default()),
            clause_counts: ClauseCounters::default(),
        });
        let finished = Cancel::default();
        let (command_tx, command_rx) = bounded(CONTROL_CAPACITY);
        let (audio_tx, audio_rx) = bounded(ASR_CAPACITY);
        let (front_tx, front_rx) = bounded(CONTROL_CAPACITY);
        let (asr_tx, asr_rx) = bounded(ASR_CAPACITY);
        let (refine_tx, refine_rx) = bounded(REFINE_CAPACITY);
        let (ready_tx, ready_rx) = bounded(1);
        let mut handle = PipelineHandle {
            commands: command_tx,
            shared: shared.clone(),
            finished: finished.clone(),
            bus: bus.clone(),
            threads: Vec::with_capacity(4),
        };
        bus.publish(PipelineEvent::ListeningState {
            state: ListeningStateKind::Starting,
        });

        let Engines {
            asr,
            second_pass,
            translator,
        } = engines;
        let asr_shared = shared.clone();
        let asr_bus = bus.clone();
        let asr_notices = asr::AsrNotices {
            notices: front_tx.clone(),
        };
        let asr_outputs = asr::AsrOutputs {
            partials: asr_tx.clone(),
            clauses: refine_tx,
            context_s: second_pass
                .as_ref()
                .map_or(0.0, |engine| engine.context_s()),
        };
        let asr_thread = spawn("lt-asr", move || {
            asr::asr_loop(
                asr,
                audio_rx,
                asr_outputs,
                &asr_bus,
                asr_shared,
                asr_notices,
            )
        })?;
        handle.threads.push(asr_thread);

        let refine_shared = shared.clone();
        let refine_thread = spawn("lt-refine", move || {
            refine::refine_loop(second_pass, refine_rx, asr_tx, refine_shared)
        })?;
        handle.threads.push(refine_thread);

        let scheduler_shared = shared.clone();
        let scheduler_bus = bus.clone();
        let scheduler_thread = spawn("lt-scheduler", move || {
            let _finished = Finished(finished);
            scheduler::scheduler_loop(scheduler::SchedulerContext {
                translator,
                front: front_rx,
                asr: asr_rx,
                shared: scheduler_shared,
                bus: scheduler_bus,
                sampler,
            })
        })?;
        handle.threads.push(scheduler_thread);

        let front_shared = shared;
        let frontend_thread = spawn("lt-frontend", move || {
            frontend::frontend_loop(frontend::FrontendContext {
                source,
                commands: command_rx,
                audio: audio_tx,
                notices: front_tx,
                bus: bus.clone(),
                shared: front_shared,
                ready: ready_tx,
            })
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

fn spawn(
    name: &str,
    work: impl FnOnce() -> Result<()> + Send + 'static,
) -> Result<JoinHandle<Result<()>>> {
    Ok(thread::Builder::new().name(name.into()).spawn(work)?)
}

struct Finished(Cancel);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.set();
    }
}

fn send_cooperative<T>(sender: &Sender<T>, mut message: T, cancelled: &Cancel) -> Result<()> {
    loop {
        if cancelled.is_set() {
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
