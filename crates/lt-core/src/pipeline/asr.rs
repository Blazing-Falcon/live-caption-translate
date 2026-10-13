use super::{
    refine::{RefineInput, Window},
    send_cooperative, ClauseCounters, FrontendNotice, POLL,
};
use crate::{
    bus::EventBus,
    config::Config,
    engines::StreamingAsr,
    error::Result,
    events::{EngineKind, EngineState, PipelineEvent},
    recognizer::{AsrMessage, Recognizer, RecognizerInput, RecognizerOutput},
    types::{StreamTime, SAMPLE_RATE},
};
use crossbeam_channel::{Receiver, Sender};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

/// The audio the second pass may read. The longest clause is the 30 s wait cap; this also
/// holds its margins.
const RING_S: u64 = 45;
/// The second pass reads a clause with this much audio before it: the clause start is an
/// estimate from the first word's late token time.
const WINDOW_BEFORE_S: f64 = 0.3;
/// And this much after it, as far as it has been heard: a commit cut lands late, so the last
/// words can lie after it.
const WINDOW_AFTER_S: f64 = 0.8;

/// What the recognizer needs to report clauses itself.
pub(super) struct AsrNotices {
    pub(super) notices: Sender<FrontendNotice>,
    pub(super) clause_counts: Arc<ClauseCounters>,
}

/// Where the recognizer's messages go: live text straight to the scheduler, closed clauses
/// through the second pass, in order.
pub(super) struct AsrOutputs {
    pub(super) partials: Sender<AsrMessage>,
    pub(super) clauses: Sender<RefineInput>,
    /// Keep audio for the second pass.
    pub(super) keep_audio: bool,
}

/// The newest audio on the stream clock. A gap or a break starts it over, so a window never
/// spans a break.
#[derive(Default)]
struct AudioRing {
    start: StreamTime,
    samples: VecDeque<f32>,
}

impl AudioRing {
    fn end(&self) -> StreamTime {
        StreamTime(self.start.0 + self.samples.len() as u64)
    }

    fn push(&mut self, at: StreamTime, samples: &[f32]) {
        if at != self.end() {
            self.clear(at);
        }
        self.samples.extend(samples);
        let excess = self
            .samples
            .len()
            .saturating_sub((RING_S * SAMPLE_RATE) as usize);
        self.samples.drain(..excess);
        self.start = StreamTime(self.start.0 + excess as u64);
    }

    fn clear(&mut self, at: StreamTime) {
        self.samples.clear();
        self.start = at;
    }

    /// The audio from `start` to `end`, cut to what the ring holds.
    fn window(&self, start: StreamTime, end: StreamTime) -> Window {
        let from = start.max(self.start).min(self.end());
        let to = end.min(self.end()).max(from);
        let offset = (from.0 - self.start.0) as usize;
        let count = (to.0 - from.0) as usize;
        Window {
            start: from,
            samples: self
                .samples
                .range(offset..offset + count)
                .copied()
                .collect(),
        }
    }
}

pub(super) fn asr_loop(
    mut asr: Box<dyn StreamingAsr>,
    config: &Config,
    inputs: Receiver<RecognizerInput>,
    outputs: AsrOutputs,
    bus: &EventBus,
    cancelled: &AtomicBool,
    notices: AsrNotices,
) -> Result<()> {
    bus.publish(PipelineEvent::EngineStatus {
        engine: EngineKind::Asr,
        state: EngineState::Ready,
        message: None,
    });
    let mut recognizer = Recognizer::new(config);
    let mut ring = AudioRing::default();
    let mut produced = Vec::new();
    while !cancelled.load(Ordering::Acquire) {
        let input = match inputs.recv_timeout(POLL) {
            Ok(input) => input,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        let break_at = match &input {
            RecognizerInput::Audio { at, samples } => {
                if outputs.keep_audio {
                    ring.push(*at, samples);
                }
                None
            }
            RecognizerInput::Break { at, .. } => Some(*at),
        };
        recognizer.handle(asr.as_mut(), input, &mut produced);
        for item in produced.drain(..) {
            match item {
                RecognizerOutput::Started(id, at) => {
                    send_cooperative(&notices.notices, FrontendNotice::Started(id), cancelled)?;
                    bus.publish(PipelineEvent::SpeechStarted {
                        id,
                        at_ms: at.millis(),
                    });
                }
                RecognizerOutput::Closed(id, end, cut) => {
                    notices.clause_counts.record(cut);
                    send_cooperative(
                        &notices.notices,
                        FrontendNotice::ClauseClosed(id, end),
                        cancelled,
                    )?;
                }
                RecognizerOutput::Message(AsrMessage::Transcript(transcript)) => {
                    let window = ring.window(
                        StreamTime::from_seconds(
                            (transcript.timing.start.seconds() - WINDOW_BEFORE_S).max(0.0),
                        ),
                        StreamTime::from_seconds(transcript.timing.end.seconds() + WINDOW_AFTER_S),
                    );
                    let clause = RefineInput::Clause {
                        transcript,
                        window,
                        closed: Instant::now(),
                    };
                    send_cooperative(&outputs.clauses, clause, cancelled)?;
                }
                RecognizerOutput::Message(message @ AsrMessage::Partial { .. }) => {
                    send_cooperative(&outputs.partials, message, cancelled)?
                }
                RecognizerOutput::Message(message) => {
                    send_cooperative(&outputs.clauses, RefineInput::Message(message), cancelled)?
                }
            }
        }
        // The clause that the break ended has its window; later audio starts over.
        if let Some(at) = break_at {
            ring.clear(at);
        }
    }
    Ok(())
}
