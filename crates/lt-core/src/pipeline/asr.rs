use super::{send_cooperative, ClauseCounters, FrontendNotice, POLL};
use crate::{
    bus::EventBus,
    config::Config,
    engines::StreamingAsr,
    error::Result,
    events::{EngineKind, EngineState, PipelineEvent},
    recognizer::{AsrMessage, Recognizer, RecognizerInput, RecognizerOutput},
};
use crossbeam_channel::{Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// What the recognizer needs to report clauses itself.
pub(super) struct AsrNotices {
    pub(super) notices: Sender<FrontendNotice>,
    pub(super) clause_counts: Arc<ClauseCounters>,
}

pub(super) fn asr_loop(
    mut asr: Box<dyn StreamingAsr>,
    config: &Config,
    inputs: Receiver<RecognizerInput>,
    output: Sender<AsrMessage>,
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
                RecognizerOutput::Message(message) => {
                    send_cooperative(&output, message, cancelled)?
                }
            }
        }
    }
    Ok(())
}
