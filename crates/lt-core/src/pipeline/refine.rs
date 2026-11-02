//! `lt-refine`: the second pass and the filter for each closed clause, in order. It owns the
//! offline recognizer and is the only path to the translator.

use super::{send_cooperative, Shared, POLL};
use crate::{
    engines::OfflineAsr,
    error::Result,
    events::DropReason,
    recognizer::AsrMessage,
    refine::trim,
    text::{classify, clean, drop_reason},
    types::{SecondPass, SecondPassOutcome, StreamTime, Transcript},
};
use crossbeam_channel::{Receiver, Sender};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// A clause that waits longer than this for the second pass keeps its live text, so the
/// queue catches up when the PC is busy.
const MAX_WAIT: Duration = Duration::from_millis(1_500);

/// The audio of a clause with its margins.
pub(super) struct Window {
    pub(super) start: StreamTime,
    pub(super) samples: Vec<f32>,
}

pub(super) enum RefineInput {
    Clause {
        transcript: Transcript,
        window: Window,
        closed: Instant,
    },
    /// A dropped or failed clause, kept in line with the others.
    Message(AsrMessage),
}

pub(super) fn refine_loop(
    mut engine: Option<Box<dyn OfflineAsr>>,
    inputs: Receiver<RefineInput>,
    output: Sender<AsrMessage>,
    shared: Arc<Shared>,
) -> Result<()> {
    let cancelled = shared.cancelled.clone();
    while !cancelled.is_set() {
        let input = match inputs.recv_timeout(POLL) {
            Ok(input) => input,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        let message = match input {
            RefineInput::Clause {
                mut transcript,
                window,
                closed,
            } => {
                let live = transcript.text.clone();
                match engine.as_mut() {
                    Some(engine) => {
                        second_pass(engine.as_mut(), &mut transcript, &window, closed, MAX_WAIT);
                    }
                    None => skip_second_pass(&mut transcript, &window, closed),
                }
                finalize(transcript, &live)
            }
            RefineInput::Message(message) => message,
        };
        send_cooperative(&output, message, &cancelled)?;
    }
    Ok(())
}

/// Replace the clause's text with the matching part of the window's offline text and record how
/// it went. A clause that waited past `max_wait` keeps the live text without a decode; a decode
/// that started in time is used even when it ends later.
fn second_pass(
    engine: &mut dyn OfflineAsr,
    transcript: &mut Transcript,
    window: &Window,
    closed: Instant,
    max_wait: Duration,
) {
    let window_end = StreamTime(window.start.0 + window.samples.len() as u64);
    let mut record = SecondPass {
        outcome: SecondPassOutcome::Late,
        live_text: None,
        window_text: None,
        window_start_ms: window.start.millis(),
        window_end_ms: window_end.millis(),
        distance: None,
        decode_ms: None,
        refine_ms: 0,
    };
    if closed.elapsed() <= max_wait {
        let started = Instant::now();
        let heard = engine.decode(&window.samples);
        record.decode_ms = Some(elapsed_ms(started));
        match heard {
            Ok(heard) => {
                match trim(&transcript.text, &heard.text) {
                    Some(trimmed) => {
                        record.outcome = SecondPassOutcome::Used;
                        record.distance = Some(u32::try_from(trimmed.distance).unwrap_or(u32::MAX));
                        transcript.text = trimmed.text;
                    }
                    None => record.outcome = SecondPassOutcome::Mismatch,
                }
                record.window_text = Some(heard.text);
            }
            Err(error) => {
                tracing::warn!(id = transcript.id.0, %error, "Second pass failed");
                record.outcome = SecondPassOutcome::Failed;
            }
        }
    }
    record.refine_ms = elapsed_ms(closed);
    transcript.second_pass = record;
}

fn skip_second_pass(transcript: &mut Transcript, window: &Window, closed: Instant) {
    let window_end = StreamTime(window.start.0 + window.samples.len() as u64);
    transcript.second_pass = SecondPass {
        outcome: SecondPassOutcome::Off,
        live_text: None,
        window_text: None,
        window_start_ms: window.start.millis(),
        window_end_ms: window_end.millis(),
        distance: None,
        decode_ms: None,
        refine_ms: elapsed_ms(closed),
    };
}

fn elapsed_ms(since: Instant) -> u32 {
    u32::try_from(since.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// Clean, classify and filter a closed clause on its final text. `live` is the recognizer's
/// text, recorded when the final text differs from it.
fn finalize(mut transcript: Transcript, live: &str) -> AsrMessage {
    transcript.text = clean(&transcript.text);
    let live = clean(live);
    transcript.second_pass.live_text = (live != transcript.text).then_some(live);
    let Some(class) = classify(&transcript.text) else {
        return AsrMessage::Dropped(transcript.id, DropReason::Empty);
    };
    transcript.class = class;
    let duration = transcript
        .timing
        .end
        .saturating_sub(transcript.timing.start);
    match drop_reason(&transcript, duration.seconds()) {
        Some(reason) => AsrMessage::Dropped(transcript.id, reason),
        None => AsrMessage::Transcript(transcript),
    }
}
