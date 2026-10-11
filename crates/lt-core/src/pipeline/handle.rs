use super::{Command, POLL};
use crate::{
    bus::{EventBus, EventReceiver},
    error::{Error, Result},
    source::AudioSource,
    types::{CutReason, PipelineStats},
};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
};

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
pub(super) struct SegmentCounters([AtomicU64; 4]);

impl SegmentCounters {
    pub(super) fn record(&self, reason: CutReason) {
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
    pub(super) commands: Sender<Command>,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) finished: Arc<AtomicBool>,
    pub(super) bus: EventBus,
    pub(super) threads: Vec<JoinHandle<Result<()>>>,
    pub(super) latest_stats: Arc<Mutex<PipelineStats>>,
    pub(super) segment_counts: Arc<SegmentCounters>,
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
