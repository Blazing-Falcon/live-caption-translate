//! `lt-refine`: the second pass and the filter for each closed clause, in order. It owns the
//! offline recognizer. Every closed clause goes through here, so this is the only path to the
//! translator.

use super::{send_cooperative, POLL};
use crate::{
    config::Config,
    engines::OfflineAsr,
    error::Result,
    events::DropReason,
    recognizer::AsrMessage,
    refine::trim,
    text::{chinese_chars, classify, clean, drop_reason},
    types::{SecondPass, SecondPassOutcome, StreamTime, TextClass, Transcript},
};
use crossbeam_channel::{Receiver, Sender};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Languages that the second pass can tag when it mishears Chinese.
const MISTAKEN_FOR_CHINESE: [&str; 3] = ["ja", "ko", "yue"];

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
    config: &Config,
    inputs: Receiver<RefineInput>,
    output: Sender<AsrMessage>,
    cancelled: &AtomicBool,
) -> Result<()> {
    let max_wait = Duration::from_secs_f32(config.asr.second_pass_max_wait_s);
    while !cancelled.load(Ordering::Acquire) {
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
                if let Some(engine) = engine.as_deref_mut() {
                    second_pass(engine, &mut transcript, &window, closed, max_wait);
                }
                finalize(transcript, &live, config)
            }
            RefineInput::Message(message) => message,
        };
        send_cooperative(&output, message, cancelled)?;
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
        let chinese = chinese_chars(&transcript.text) > 0;
        let heard =
            engine
                .decode(&window.samples, None)
                .and_then(|heard| match heard.lang.as_deref() {
                    Some(lang) if chinese && MISTAKEN_FOR_CHINESE.contains(&lang) => {
                        engine.decode(&window.samples, Some("zh"))
                    }
                    _ => Ok(heard),
                });
        record.decode_ms = Some(elapsed_ms(started));
        match heard {
            Ok(heard) => {
                match trim(&transcript.text, &heard.text) {
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
    transcript.second_pass = Some(record);
}

fn elapsed_ms(since: Instant) -> u32 {
    since.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

/// Clean, classify and filter a closed clause on its final text. `live` is the recognizer's
/// text, recorded when the final text differs from it.
fn finalize(mut transcript: Transcript, live: &str, config: &Config) -> AsrMessage {
    transcript.text = clean(&transcript.text, &config.filter.fillers);
    if let Some(record) = transcript.second_pass.as_mut() {
        let live = clean(live, &config.filter.fillers);
        record.live_text = (live != transcript.text).then_some(live);
    }
    let Some(class) = classify(&transcript.text) else {
        return AsrMessage::Dropped(transcript.id, DropReason::Empty);
    };
    transcript.class = class;
    transcript.lang_tag = if class == TextClass::Other {
        other_language(&transcript.text)
    } else {
        text_language(&transcript.text)
    };
    let duration = transcript
        .timing
        .end
        .saturating_sub(transcript.timing.start);
    match drop_reason(&transcript, duration.seconds(), &config.filter) {
        Some(reason) => AsrMessage::Dropped(transcript.id, reason),
        None => AsrMessage::Transcript(transcript),
    }
}

/// Japanese or Korean, from the kana and Hangul in the text.
fn other_language(text: &str) -> Option<String> {
    let kana = text
        .chars()
        .filter(|character| matches!(character, '\u{3040}'..='\u{30ff}'))
        .count();
    let hangul = text
        .chars()
        .filter(|character| matches!(character, '\u{ac00}'..='\u{d7af}'))
        .count();
    if hangul > kana {
        Some("ko".into())
    } else if kana > 0 {
        Some("ja".into())
    } else {
        None
    }
}

/// Language from the share of Latin letters and CJK characters; the live engine gives no tag.
fn text_language(text: &str) -> Option<String> {
    let latin = text.chars().filter(char::is_ascii_alphabetic).count();
    let cjk = chinese_chars(text);
    match (latin, cjk) {
        (0, 0) => None,
        _ if latin > cjk => Some("en".into()),
        _ => Some("zh".into()),
    }
}
