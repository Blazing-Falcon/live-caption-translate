use super::{send_cooperative, FrontendNotice, SegmentCounters, POLL};
use crate::{
    bus::EventBus,
    config::Config,
    engines::StreamingAsr,
    error::Result,
    events::{EngineKind, EngineState, PipelineEvent},
    stream::{AsrMessage, StreamInput, StreamOutput, StreamRecognizer},
};
use crossbeam_channel::{Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// What the recognizer needs to report clauses itself.
pub(super) struct AsrNotices {
    pub(super) notices: Sender<FrontendNotice>,
    pub(super) segment_counts: Arc<SegmentCounters>,
}

pub(super) fn asr_loop(
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
                    send_cooperative(&stream.notices, FrontendNotice::Started(id), cancelled)?;
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
