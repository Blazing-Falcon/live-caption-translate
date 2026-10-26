use super::{
    queue::{QueuedTranscript, TranslationQueue},
    running::RunningText,
    send_cooperative,
    translator::TranslatorWorker,
    FrontendNotice, Shared, CONTROL_CAPACITY, POLL, QUEUE_CAPACITY,
};
use crate::{
    bus::EventBus,
    clock::Clock,
    engines::Translator,
    error::Result,
    events::{DropReason, EngineKind, EngineState, FailReason, PipelineEvent},
    metrics::{Metrics, ResourceSample, ResourceSampler},
    recognizer::AsrMessage,
    types::{StreamTime, TextClass, Transcript, UtteranceId},
};
use crossbeam_channel::{Receiver, TryRecvError};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{atomic::Ordering, Arc},
    thread,
    time::{Duration, Instant},
};

pub(super) struct SchedulerContext {
    pub(super) translator: Box<dyn Translator>,
    pub(super) front: Receiver<FrontendNotice>,
    pub(super) asr: Receiver<AsrMessage>,
    pub(super) shared: Arc<Shared>,
    pub(super) bus: EventBus,
    pub(super) sampler: Option<Box<dyn ResourceSampler>>,
}

/// What one step of the loop did.
#[derive(Clone, Copy, PartialEq)]
enum Step {
    Idle,
    Progressed,
    Stop,
}

impl Step {
    fn progressed(self) -> bool {
        self == Self::Progressed
    }
}

struct Scheduler {
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
    shared: Arc<Shared>,
    asr_failed: bool,
    running_text: RunningText,
    /// The lines the preview in flight covers.
    previewing: Option<Vec<UtteranceId>>,
    front: Receiver<FrontendNotice>,
    asr: Receiver<AsrMessage>,
    worker: TranslatorWorker,
    front_done: bool,
    asr_done: bool,
    pending_asr: Option<AsrMessage>,
    last_stats: Instant,
    was_listening: bool,
}

impl Scheduler {
    fn translate_unit(&mut self, unit: Transcript) {
        for id in &unit.absorbed {
            self.observed.remove(id);
            self.translating.remove(id);
        }
        let mut ids = Vec::with_capacity(unit.absorbed.len() + 1);
        ids.push(unit.id);
        ids.extend(unit.absorbed.iter().copied());
        self.publish(PipelineEvent::UnitSealed {
            id: unit.id,
            ids,
            text: unit.text.clone(),
        });
        self.ready.push_back(unit);
    }

    fn freeze_running(&mut self) {
        if let Some(unit) = self.running_text.freeze() {
            self.translate_unit(unit);
        }
    }
    fn publish(&mut self, event: PipelineEvent) {
        self.metrics.record(&event);
        self.bus.publish(event);
    }

    fn publish_asr_final(&mut self, transcript: &Transcript) {
        self.publish(PipelineEvent::AsrFinal {
            id: transcript.id,
            text: transcript.text.clone(),
            class: transcript.class,
            start_ms: transcript.timing.start.millis(),
            end_ms: transcript.timing.end.millis(),
            asr_ms: transcript.timing.asr_ms,
            cut: transcript.cut,
            logprob: transcript.logprob,
            second_pass: transcript.second_pass.clone(),
        });
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
        let snapshot = self.metrics.snapshot(live, waiting);
        if let Ok(mut latest) = self.shared.latest_stats.lock() {
            *latest = snapshot;
        }
    }

    fn notice(&mut self, notice: FrontendNotice) {
        match notice {
            FrontendNotice::Started(id) => {
                self.started_through =
                    Some(self.started_through.map_or(id, |previous| previous.max(id)));
                if self.handled_through.is_none_or(|handled| id > handled) {
                    self.observed.insert(id);
                }
            }
            FrontendNotice::Discontinuity(at) => self.queue.discontinuity(at),
            FrontendNotice::ClauseClosed(id, end) => {
                self.closed_through =
                    Some(self.closed_through.map_or(id, |previous| previous.max(id)));
                self.metrics.clause_closed(id, end);
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
                // Only Chinese text is translated, so only it may join a preview; a preview
                // that carried heard English would send English to the translator.
                if class.is_translated() {
                    self.running_text.partial(id, text.clone());
                }
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
                self.running_text.closed(transcript.id);
                self.observed.insert(transcript.id);
                if transcript.class.is_translated() {
                    self.publish_asr_final(&transcript);
                    self.translating.insert(transcript.id);
                    for unit in self.running_text.push(transcript) {
                        self.translate_unit(unit);
                    }
                } else {
                    // A line shown as heard seals the running unit before its own final, so the
                    // unit's events never follow the line that ended it.
                    self.freeze_running();
                    self.publish_asr_final(&transcript);
                    self.observed.remove(&transcript.id);
                }
            }
            AsrMessage::Dropped(id, reason) => {
                self.running_text.closed(id);
                self.observed.remove(&id);
                self.publish(PipelineEvent::Dropped { id, reason });
            }
            AsrMessage::Partial { .. } => {}
            AsrMessage::Failed(id, message) => {
                // There is no ASR-failure line type in the wire contract; the
                // engine error is explicit and no nonexistent ASR text is sent.
                self.running_text.closed(id);
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
            }
        }
    }

    fn completed(&mut self, event: Option<PipelineEvent>) {
        self.in_flight = false;
        if let Some(ids) = self.previewing.take() {
            self.running_text.preview_done(Instant::now());
            if let Some(event) = event.filter(|_| self.running_text.current(&ids)) {
                self.publish(event);
            }
            return;
        }
        let Some(event) = event else {
            return;
        };
        if let PipelineEvent::TranslationFinal { id, .. }
        | PipelineEvent::TranslationFailed { id, .. } = &event
        {
            self.observed.remove(id);
            self.translating.remove(id);
        }
        self.publish(event);
    }

    /// Running text: translate the lines not yet translated for good again, when the
    /// translator has nothing else to do.
    fn next_preview(&mut self, queued_ms: u64) -> Option<QueuedTranscript> {
        if !self.ready.is_empty() || !self.queue.is_empty() {
            return None;
        }
        let (transcript, ids) = self.running_text.preview(Instant::now())?;
        self.previewing = Some(ids.clone());
        Some(QueuedTranscript {
            transcript,
            queued_ms,
            preview: Some(ids),
        })
    }

    /// Frontend notices: starts, breaks and clause closes.
    fn drain_notices(&mut self) -> Step {
        let mut step = Step::Idle;
        for _ in 0..CONTROL_CAPACITY * 2 {
            match self.front.try_recv() {
                Ok(notice) => {
                    step = Step::Progressed;
                    self.notice(notice);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.front_done = true;
                    break;
                }
            }
        }
        step
    }

    /// Finished translations, in the order the worker sent them.
    fn drain_completions(&mut self) -> Step {
        let mut step = Step::Idle;
        loop {
            match self.worker.completed.try_recv() {
                Ok(event) => {
                    step = Step::Progressed;
                    self.completed(event);
                }
                Err(TryRecvError::Empty) => return step,
                Err(TryRecvError::Disconnected) => {
                    self.translator_lost();
                    return Step::Stop;
                }
            }
        }
    }

    /// Move sealed units into the translation queue while it has room.
    fn feed_queue(&mut self, now: StreamTime) -> Result<Step> {
        let mut step = Step::Idle;
        while self.queue.len() < QUEUE_CAPACITY {
            let Some(transcript) = self.ready.pop_front() else {
                break;
            };
            self.queue.push(transcript, now.millis())?;
            step = Step::Progressed;
        }
        Ok(step)
    }

    /// Read ASR while translation has room; a line whose notices have not arrived waits.
    fn read_asr(&mut self) -> Step {
        if self.asr_done || !self.ready.is_empty() || self.queue.len() >= QUEUE_CAPACITY {
            return Step::Idle;
        }
        let message = if let Some(message) = self.pending_asr.take() {
            Some(message)
        } else {
            match self.asr.try_recv() {
                Ok(message) => Some(message),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    self.asr_done = true;
                    return Step::Progressed;
                }
            }
        };
        let Some(message) = message else {
            return Step::Idle;
        };
        // The notice and transcript channels have independent senders. A line's start and
        // close notices come first, so a terminal event at shutdown and the lag stat know the
        // line. A partial only needs its start notice.
        let started = self.started_through.is_some_and(|id| id >= message.id());
        let closed = self.closed_through.is_some_and(|id| id >= message.id());
        let is_partial = matches!(message, AsrMessage::Partial { .. });
        if self.front_done || (started && (closed || is_partial)) {
            self.transcript(message);
            Step::Progressed
        } else {
            self.pending_asr = Some(message);
            Step::Idle
        }
    }

    /// Send the next queued unit, or a running-text preview when nothing is queued.
    fn dispatch(&mut self, now: StreamTime) -> Result<Step> {
        let cancelled = self.shared.cancelled.clone();
        if !self.in_flight && !self.queue.is_empty() {
            let batch = self.queue.take(now);
            for event in batch.events {
                match &event {
                    PipelineEvent::Skipped { id, .. } => {
                        self.observed.remove(id);
                        self.translating.remove(id);
                    }
                    PipelineEvent::UnitSealed {
                        id: leader, ids, ..
                    } => {
                        for id in ids.iter().filter(|id| *id != leader) {
                            self.observed.remove(id);
                            self.translating.remove(id);
                        }
                    }
                    _ => {}
                }
                self.publish(event);
            }
            if let Some(item) = batch.item {
                // Its id stays observed, so the shutdown below settles it either way.
                if send_cooperative(&self.worker.input, item, &cancelled).is_err() {
                    if !cancelled.is_set() {
                        self.translator_lost();
                    }
                    return Ok(Step::Stop);
                }
                self.in_flight = true;
                return Ok(Step::Progressed);
            }
        }
        if !self.in_flight {
            if let Some(item) = self.next_preview(now.millis()) {
                if send_cooperative(&self.worker.input, item, &cancelled).is_err() {
                    if !cancelled.is_set() {
                        self.translator_lost();
                    }
                    return Ok(Step::Stop);
                }
                self.in_flight = true;
                return Ok(Step::Progressed);
            }
        }
        Ok(Step::Idle)
    }

    fn is_drained(&self) -> bool {
        self.asr_done
            && self.front_done
            && self.ready.is_empty()
            && self.queue.is_empty()
            && !self.in_flight
    }

    /// Publish Stats once a second while listening, never across the pause boundary.
    fn publish_stats(&mut self) {
        let is_listening = self.shared.listening.load(Ordering::Acquire);
        if is_listening != self.was_listening {
            self.was_listening = is_listening;
            self.last_stats = Instant::now();
        }
        if !is_listening || self.last_stats.elapsed() < Duration::from_secs(1) {
            return;
        }
        // Frontend clears listening under this lock before acknowledging pause. A timer
        // cannot publish a stale Stats after that boundary.
        if let Ok(latest) = self.shared.latest_stats.lock() {
            if self.shared.listening.load(Ordering::Acquire) && !self.shared.cancelled.is_set() {
                self.bus.publish(PipelineEvent::Stats(latest.clone()));
            }
        }
        self.last_stats = Instant::now();
    }

    /// The translator thread ended by itself, which only a panic does: report it and stop the
    /// session, whose shutdown settles every line.
    fn translator_lost(&self) {
        self.bus.publish(PipelineEvent::EngineStatus {
            engine: EngineKind::Translator,
            state: EngineState::Failed,
            message: Some("The translator stopped unexpectedly".into()),
        });
        self.shared.cancelled.set();
    }

    /// Join the worker, wait for the producers to close, and settle every line.
    fn shutdown(&mut self, clock: &dyn Clock) -> Result<()> {
        // A worker that panicked reports it after every line is settled.
        let stopped = self.worker.stop();
        // Wait for producers to close before publishing shutdown terminals. This prevents a
        // racing SpeechStarted or delta from appearing after a terminal.
        while !self.front_done || !self.asr_done {
            if !self.front_done {
                match self.front.recv_timeout(POLL) {
                    Ok(FrontendNotice::Started(id)) => {
                        if self.handled_through.is_none_or(|handled| id > handled) {
                            self.observed.insert(id);
                        }
                    }
                    Ok(FrontendNotice::ClauseClosed(id, end)) => {
                        self.metrics.clause_closed(id, end);
                    }
                    Ok(_) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        self.front_done = true
                    }
                }
            }
            if !self.asr_done {
                match self.asr.recv_timeout(POLL) {
                    Ok(_) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => self.asr_done = true,
                }
            }
        }
        let finished: Vec<_> = self.worker.completed.try_iter().collect();
        for event in finished {
            self.completed(event);
        }
        let unfinished: Vec<_> = self.observed.iter().copied().collect();
        for id in unfinished {
            self.publish(if self.translating.contains(&id) {
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
        self.ready.clear();
        self.queue = TranslationQueue::default();
        self.in_flight = false;
        self.refresh_stats(clock.now());
        stopped
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

pub(super) fn scheduler_loop(context: SchedulerContext) -> Result<()> {
    let SchedulerContext {
        translator,
        front,
        asr,
        shared,
        bus,
        sampler,
    } = context;
    let cancelled = shared.cancelled.clone();
    let clock = shared.clock.clone();
    let mut metrics = Metrics::new();
    metrics.set_sampler(sampler.map(|inner| {
        Box::new(CachedSampler {
            inner,
            at: None,
            last: ResourceSample::default(),
        }) as Box<dyn ResourceSampler>
    }));
    let worker =
        TranslatorWorker::start(translator, clock.clone(), bus.clone(), cancelled.clone())?;
    let mut scheduler = Scheduler {
        queue: TranslationQueue::default(),
        ready: VecDeque::new(),
        bus,
        observed: BTreeSet::new(),
        translating: BTreeSet::new(),
        in_flight: false,
        started_through: None,
        handled_through: None,
        closed_through: None,
        metrics,
        shared: shared.clone(),
        asr_failed: false,
        running_text: RunningText::default(),
        previewing: None,
        front,
        asr,
        worker,
        front_done: false,
        asr_done: false,
        pending_asr: None,
        last_stats: Instant::now(),
        was_listening: shared.listening.load(Ordering::Acquire),
    };
    scheduler.refresh_stats(clock.now());
    while !cancelled.is_set() {
        let mut progressed = scheduler.drain_notices().progressed();
        if scheduler.drain_completions() == Step::Stop {
            break;
        }
        if scheduler.asr_done && scheduler.front_done {
            scheduler.freeze_running();
        }
        progressed |= scheduler.feed_queue(clock.now())?.progressed();
        progressed |= scheduler.read_asr().progressed();
        let step = scheduler.dispatch(clock.now())?;
        if step == Step::Stop {
            break;
        }
        progressed |= step.progressed();
        if scheduler.is_drained() {
            break;
        }
        scheduler.refresh_stats(clock.now());
        scheduler.publish_stats();
        // Available messages wake the stage without an artificial frame/ASR
        // throttle. Idle polling remains bounded and does not spin a CPU core.
        if !progressed {
            thread::sleep(POLL);
        }
    }
    scheduler.shutdown(clock.as_ref())
}
