//! `lt-refine`: the second pass and the filter for each closed clause, in order. It owns the
//! offline recognizer. Every closed clause goes through here, so this is the only path to the
//! translator.

use super::{send_cooperative, Shared, POLL};
use crate::{
    engines::OfflineAsr,
    error::Result,
    events::DropReason,
    recognizer::AsrMessage,
    refine::{cut_at_next_clause, trim},
    text::{classify, clean, drop_reason},
    types::{SecondPass, SecondPassOutcome, StreamTime, Transcript, UtteranceId},
};
use crossbeam_channel::{Receiver, Sender};
use std::{
    sync::{Arc, Mutex},
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

/// The live text of the clause that is open right now, shared with the second pass.
///
/// A window runs past its clause, so its offline text can end with the start of the next
/// clause. That text is only known once the next clause has started, which is after this one
/// closed; the asr thread writes it here and the second pass reads it after its decode.
#[derive(Default)]
pub(super) struct OpenClause(Mutex<Option<(UtteranceId, String)>>);

impl OpenClause {
    /// Record the open clause's newest live text.
    pub(super) fn heard(&self, id: UtteranceId, text: &str) {
        if let Ok(mut open) = self.0.lock() {
            *open = Some((id, text.to_owned()));
        }
    }

    /// The live text of the clause right after `id`, when the open clause is that one. Text
    /// from any other clause must not cut a window: the cell can still hold the clause that
    /// just closed, or a later one when the second pass has fallen behind.
    fn after(&self, id: UtteranceId) -> Option<String> {
        let open = self.0.lock().ok()?;
        let (open_id, text) = open.as_ref()?;
        let next = id.0.checked_add(1) == Some(open_id.0);
        (next && !text.is_empty()).then(|| text.clone())
    }
}

pub(super) enum RefineInput {
    Clause {
        transcript: Transcript,
        window: Window,
        closed: Instant,
        /// The clause that is open while this one is refined.
        open: Arc<OpenClause>,
    },
    /// A dropped or failed clause, kept in line with the others.
    Message(AsrMessage),
}

pub(super) fn refine_loop(
    mut engine: Box<dyn OfflineAsr>,
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
                open,
            } => {
                let live = transcript.text.clone();
                second_pass(
                    engine.as_mut(),
                    &mut transcript,
                    &window,
                    &open,
                    closed,
                    MAX_WAIT,
                );
                finalize(transcript, &live)
            }
            RefineInput::Message(message) => message,
        };
        send_cooperative(&output, message, &cancelled)?;
    }
    Ok(())
}

/// Replace the clause's text with the matching part of the window's offline text, and record
/// how it went. A clause that waited past `max_wait` keeps the live text without a decode, so
/// the queue catches up when the engine is slow. A decode that started in time is used even if
/// it ends later: the wait is spent by then.
fn second_pass(
    engine: &mut dyn OfflineAsr,
    transcript: &mut Transcript,
    window: &Window,
    open: &OpenClause,
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
                // The next clause has usually started by now, and the window ends in its first
                // words. Those words are not this clause's, so `trim` must not choose them.
                let next = open.after(transcript.id);
                let text = next
                    .as_deref()
                    .and_then(|next| cut_at_next_clause(&heard.text, next))
                    .unwrap_or(&heard.text);
                if text.len() != heard.text.len() {
                    tracing::debug!(
                        id = transcript.id.0,
                        dropped = heard.text.len() - text.len(),
                        "Second-pass window ends in the next clause"
                    );
                }
                match trim(&transcript.text, text) {
                    Some(trimmed) => {
                        record.outcome = SecondPassOutcome::Used;
                        record.distance = Some(trimmed.distance.min(u32::MAX as usize) as u32);
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

fn elapsed_ms(since: Instant) -> u32 {
    since.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The refine thread reads the cell after clause `id` closed. Only the clause that opened
    /// after it may cut its window or mark it unfinished; the clause itself, or the one before
    /// it when this clause produced no partial, must not.
    #[test]
    fn only_the_next_clause_is_read_as_what_follows() {
        let open = OpenClause::default();
        assert_eq!(open.after(UtteranceId(5)), None);
        open.heard(UtteranceId(4), "上一句");
        assert_eq!(open.after(UtteranceId(5)), None);
        open.heard(UtteranceId(5), "这一句");
        assert_eq!(open.after(UtteranceId(5)), None);
        open.heard(UtteranceId(6), "而且我还有事");
        assert_eq!(open.after(UtteranceId(5)).as_deref(), Some("而且我还有事"));
        open.heard(UtteranceId(7), "再下一句");
        assert_eq!(open.after(UtteranceId(5)), None);
    }
}
