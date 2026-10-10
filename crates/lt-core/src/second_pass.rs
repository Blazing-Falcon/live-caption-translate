//! The second recognizer decodes each final clause again on its own owner thread.
//!
//! The input channel holds one clause, so a slow engine makes the scheduler skip clauses
//! (`busy`) instead of building a backlog. A native decode cannot be interrupted: a clause whose
//! deadline passes in the queue is not decoded, and a late result is ignored by the scheduler.

use crate::{
    clock::Clock,
    engines::{TokenTranscript, WindowAsr},
    error::{Error, Result},
    source::SessionClock,
    types::UtteranceId,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(5);
/// One clause waits while one decodes.
const INPUT_CAPACITY: usize = 1;
/// Started and Done for the clause in the engine, plus one expiry.
const OUTPUT_CAPACITY: usize = 4;

pub(crate) struct Job {
    pub id: UtteranceId,
    pub samples: Arc<[f32]>,
    pub deadline: Instant,
}

pub(crate) enum Outcome {
    Started {
        id: UtteranceId,
        at_ms: u64,
    },
    Done {
        id: UtteranceId,
        result: Result<TokenTranscript>,
        at_ms: u64,
    },
    /// The deadline passed before the engine was free.
    Expired {
        id: UtteranceId,
    },
}

pub(crate) struct SecondPassWorker {
    input: Option<Sender<Job>>,
    pub completed: Receiver<Outcome>,
    thread: Option<JoinHandle<()>>,
}

impl SecondPassWorker {
    /// `setup` runs first on the new thread (the app lowers its priority there).
    pub fn start(
        mut engine: Box<dyn WindowAsr>,
        setup: Option<fn()>,
        clock: Arc<SessionClock>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (input, jobs) = bounded::<Job>(INPUT_CAPACITY);
        let (output, completed) = bounded(OUTPUT_CAPACITY);
        let thread = thread::Builder::new()
            .name("lt-asr2".into())
            .spawn(move || {
                if let Some(setup) = setup {
                    setup();
                }
                while !cancelled.load(Ordering::Acquire) {
                    let job = match jobs.recv_timeout(POLL) {
                        Ok(job) => job,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
                    if Instant::now() >= job.deadline {
                        if !send(&output, Outcome::Expired { id: job.id }, &cancelled) {
                            break;
                        }
                        continue;
                    }
                    let started = Outcome::Started {
                        id: job.id,
                        at_ms: clock.now().millis(),
                    };
                    if !send(&output, started, &cancelled) {
                        break;
                    }
                    let span = tracing::info_span!("asr2", id = job.id.0);
                    let result = span.in_scope(|| engine.decode_window(&job.samples));
                    let done = Outcome::Done {
                        id: job.id,
                        result,
                        at_ms: clock.now().millis(),
                    };
                    if !send(&output, done, &cancelled) {
                        break;
                    }
                }
            })?;
        Ok(Self {
            input: Some(input),
            completed,
            thread: Some(thread),
        })
    }

    /// `Ok(false)` when a clause is already waiting.
    pub fn try_submit(&self, job: Job) -> Result<bool> {
        let Some(input) = self.input.as_ref() else {
            return Err(Error::Stopped);
        };
        match input.try_send(job) {
            Ok(()) => Ok(true),
            Err(crossbeam_channel::TrySendError::Full(_)) => Ok(false),
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                Err(Error::Engine("The second-pass recognizer stopped".into()))
            }
        }
    }

    /// Waits for a running native decode to return.
    pub fn stop(&mut self) -> Result<()> {
        self.input = None;
        if let Some(thread) = self.thread.take() {
            // Drain so a blocked send cannot hold the thread.
            while !thread.is_finished() {
                while self.completed.try_recv().is_ok() {}
                thread::sleep(POLL);
            }
            thread.join().map_err(|_| {
                Error::Engine("The second-pass recognizer stopped unexpectedly".into())
            })?;
        }
        Ok(())
    }
}

impl Drop for SecondPassWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn send(output: &Sender<Outcome>, mut outcome: Outcome, cancelled: &AtomicBool) -> bool {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        match output.send_timeout(outcome, POLL) {
            Ok(()) => return true,
            Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => outcome = returned,
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => return false,
        }
    }
}
