use super::{send_cooperative, FrontendNotice, CONTROL_CAPACITY, POLL};
use crate::{
    bus::EventBus,
    clock::Clock,
    config::Config,
    engines::Translator,
    error::Result,
    events::{DropReason, EngineKind, EngineState, FailReason, PipelineEvent},
    metrics::{Metrics, ResourceSample, ResourceSampler},
    queue::{TranslationQueue, TranslatorWorker},
    recognizer::AsrMessage,
    source::SessionClock,
    types::{PipelineStats, StreamTime, TextClass, Transcript, UtteranceId},
};
use crossbeam_channel::{Receiver, TryRecvError};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// Chinese and mixed lines are translated; English and other languages show as heard.
fn translates(transcript: &Transcript) -> bool {
    transcript.class.is_translated()
}

struct Scheduler {
    config: Config,
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
        let snapshot = self.metrics.snapshot(live, waiting);
        if let Ok(mut latest) = self.latest_stats.lock() {
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
                    second_pass: transcript.second_pass.clone(),
                });
                if translates(&transcript) {
                    self.translating.insert(transcript.id);
                    self.ready.push_back(transcript);
                } else {
                    self.observed.remove(&transcript.id);
                }
            }
            AsrMessage::Dropped(id, reason) => {
                self.observed.remove(&id);
                self.publish(PipelineEvent::Dropped { id, reason });
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
pub(super) fn scheduler_loop(
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
    let mut metrics = Metrics::new();
    metrics.set_sampler(sampler.map(|inner| {
        Box::new(CachedSampler {
            inner,
            at: None,
            last: ResourceSample::default(),
        }) as Box<dyn ResourceSampler>
    }));
    let mut scheduler = Scheduler {
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
                    scheduler.notice(notice);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    front_done = true;
                    break;
                }
            }
        }
        let mut worker_lost = false;
        loop {
            match worker.completed.try_recv() {
                Ok(event) => {
                    progressed = true;
                    scheduler.completed(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    worker_lost = true;
                    break;
                }
            }
        }
        if worker_lost {
            translator_lost(&scheduler, &cancelled);
            break;
        }
        while scheduler.queue.len() < TranslationQueue::CAPACITY {
            let Some(transcript) = scheduler.ready.pop_front() else {
                break;
            };
            scheduler.queue.push(transcript, clock.now().millis())?;
            progressed = true;
        }
        // Stop reading ASR when translation is saturated.
        if !asr_done
            && scheduler.ready.is_empty()
            && scheduler.queue.len() < TranslationQueue::CAPACITY
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
                // The notice and transcript channels have independent senders. A line's
                // start and close notices come first, so a terminal event at shutdown and
                // the lag stat know the line. A partial only needs its start notice.
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
                // Its id stays observed, so the shutdown below settles it either way.
                if send_cooperative(&worker.input, item, &cancelled).is_err() {
                    if !cancelled.load(Ordering::Acquire) {
                        translator_lost(&scheduler, &cancelled);
                    }
                    break;
                }
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
    // A worker that panicked reports it after every line is settled.
    let stopped = worker.stop();
    // Wait for producers to close before publishing shutdown terminals. This
    // prevents a racing SpeechStarted or delta from appearing after a terminal.
    while !front_done || !asr_done {
        if !front_done {
            match front.recv_timeout(POLL) {
                Ok(FrontendNotice::Started(id)) => {
                    if scheduler.handled_through.is_none_or(|handled| id > handled) {
                        scheduler.observed.insert(id);
                    }
                }
                Ok(FrontendNotice::ClauseClosed(id, end)) => {
                    scheduler.metrics.clause_closed(id, end);
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
    scheduler.in_flight = false;
    scheduler.refresh_stats(clock.now());
    stopped
}

/// The translator thread ended by itself, which only a panic does: report it and stop the
/// session, whose shutdown settles every line.
fn translator_lost(scheduler: &Scheduler, cancelled: &AtomicBool) {
    scheduler.bus.publish(PipelineEvent::EngineStatus {
        engine: EngineKind::Translator,
        state: EngineState::Failed,
        message: Some("The translator stopped unexpectedly".into()),
    });
    cancelled.store(true, Ordering::Release);
}
