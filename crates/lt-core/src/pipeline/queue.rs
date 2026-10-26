//! Queue policy: which waiting lines are skipped, joined or translated on their own.

use crate::{
    error::{Error, Result},
    events::{PipelineEvent, SkipReason},
    types::{StreamTime, TextClass, Transcript, UtteranceId},
};
use std::collections::VecDeque;

/// After a silence this long the talk has likely moved on, so waiting lines are not joined
/// across it.
const JOIN_GAP_S: f64 = 10.0;
/// Older waiting lines are skipped once the live audio is this far ahead.
pub(crate) const SKIP_LAG_S: f64 = 6.0;
/// The most text one request joins from the waiting lines.
const JOIN_MAX_CHARS: usize = 120;
/// Lines the queue holds before the recognizer is held back.
pub const TRANSLATION_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Debug)]
pub(super) struct QueuedTranscript {
    pub(super) transcript: Transcript,
    pub(super) queued_ms: u64,
    /// Running text: the lines this preview covers. A preview streams no deltas and ends in
    /// `TranslationPreview` or nothing.
    pub(super) preview: Option<Vec<UtteranceId>>,
}

#[derive(Default)]
pub(super) struct QueueBatch {
    pub(super) item: Option<QueuedTranscript>,
    pub(super) events: Vec<PipelineEvent>,
    pub(super) skipped: u32,
}

#[derive(Default)]
pub(super) struct TranslationQueue {
    /// Each waiting line, and whether it starts a new join group (see `new_group`).
    waiting: VecDeque<(QueuedTranscript, bool)>,
    /// Breaks after the end of the newest queued line, oldest first.
    breaks: VecDeque<StreamTime>,
    /// End of the newest queued line.
    last_end: Option<StreamTime>,
}

impl TranslationQueue {
    pub(super) fn push(&mut self, transcript: Transcript, queued_ms: u64) -> Result<()> {
        if self.waiting.len() == TRANSLATION_QUEUE_CAPACITY {
            return Err(Error::Engine("Translation queue is full".into()));
        }
        let new_group = self.new_group(&transcript);
        self.waiting.push_back((
            QueuedTranscript {
                transcript,
                queued_ms,
                preview: None,
            },
            new_group,
        ));
        Ok(())
    }

    /// Lines before a discontinuity are not joined with lines after it. Lines reach the queue
    /// after their notice, so the break is kept by time.
    pub(super) fn discontinuity(&mut self, at: StreamTime) {
        self.breaks.push_back(at);
    }

    /// A break or a long gap comes before this line, so it must not be joined with the line
    /// before it.
    fn new_group(&mut self, transcript: &Transcript) -> bool {
        let start = transcript.timing.start;
        // Every kept break is at or after the previous line's end.
        let after_break = self.breaks.front().is_some_and(|at| *at <= start);
        while self
            .breaks
            .front()
            .is_some_and(|at| *at < transcript.timing.end)
        {
            self.breaks.pop_front();
        }
        let after_gap = self
            .last_end
            .is_some_and(|end| start.saturating_sub(end).seconds() > JOIN_GAP_S);
        self.last_end = Some(transcript.timing.end);
        after_break || after_gap
    }

    pub(super) fn len(&self) -> usize {
        self.waiting.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    pub(super) fn take(&mut self, live: StreamTime) -> QueueBatch {
        let mut batch = QueueBatch::default();
        let skip = self.waiting.front().is_some_and(|(item, _)| {
            live.saturating_sub(item.transcript.timing.end).seconds() > SKIP_LAG_S
        });
        if skip {
            while self.waiting.len() > 1 {
                if let Some((old, _)) = self.waiting.pop_front() {
                    batch.events.push(PipelineEvent::Skipped {
                        id: old.transcript.id,
                        reason: SkipReason::CatchUp,
                    });
                    batch.skipped += 1;
                }
            }
        }
        let Some((mut item, _)) = self.waiting.pop_front() else {
            return batch;
        };
        let mut absorbed = false;
        while let Some((next, new_group)) = self.waiting.front() {
            if *new_group {
                break;
            }
            let joined = join_text(&item.transcript.text, &next.transcript.text);
            if joined.chars().count() > JOIN_MAX_CHARS {
                break;
            }
            let Some((next, _)) = self.waiting.pop_front() else {
                break;
            };
            absorbed = true;
            item.transcript.absorbed.push(next.transcript.id);
            item.transcript
                .absorbed
                .extend(next.transcript.absorbed.iter().copied());
            item.transcript.text = joined;
            item.transcript.timing.end = next.transcript.timing.end;
            item.transcript.timing.asr_done_ms = item
                .transcript
                .timing
                .asr_done_ms
                .max(next.transcript.timing.asr_done_ms);
            item.transcript.timing.asr_ms = item
                .transcript
                .timing
                .asr_ms
                .saturating_add(next.transcript.timing.asr_ms);
            if next.transcript.class == TextClass::Mixed {
                item.transcript.class = TextClass::Mixed;
            }
        }
        if absorbed {
            let mut ids = Vec::with_capacity(item.transcript.absorbed.len() + 1);
            ids.push(item.transcript.id);
            ids.extend(item.transcript.absorbed.iter().copied());
            batch.events.push(PipelineEvent::UnitSealed {
                id: item.transcript.id,
                ids,
                text: item.transcript.text.clone(),
            });
        }
        batch.item = Some(item);
        batch
    }
}

/// A sentence end becomes a comma at the join boundary.
fn join_text(first: &str, next: &str) -> String {
    let first = first.trim_end();
    let mut joined = match first.strip_suffix('。').or_else(|| first.strip_suffix('.')) {
        Some(prefix) => format!("{prefix}，"),
        None => first.to_owned(),
    };
    joined.push_str(next.trim_start());
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CutReason, StageTiming};

    fn transcript(id: u64, text: &str, end: f64) -> Transcript {
        Transcript {
            id: UtteranceId(id),
            text: text.into(),
            class: TextClass::Chinese,
            timing: StageTiming {
                start: StreamTime::from_seconds(end - 1.0),
                end: StreamTime::from_seconds(end),
                asr_done_ms: (end * 1000.0) as u64,
                asr_ms: 1,
            },
            absorbed: Vec::new(),
            cut: CutReason::Pause,
            logprob: None,
            second_pass: crate::types::SecondPass::default(),
        }
    }

    #[test]
    fn queue_joins_waiting_items_and_preserves_leader_and_cap_order() {
        // Two 60-character lines fill the 120-character join limit; the third starts a new request.
        let line = format!("{}。", "你".repeat(59));
        let mut queue = TranslationQueue::default();
        for id in 1..=3 {
            queue
                .push(transcript(id, &line, f64::from(id as u32)), id * 1000)
                .unwrap();
        }
        let batch = queue.take(StreamTime::from_seconds(3.0));
        let item = batch.item.unwrap();
        assert_eq!(item.transcript.id, UtteranceId(1));
        assert_eq!(item.transcript.text, line.replacen('。', "，", 1) + &line);
        assert_eq!(item.transcript.absorbed, vec![UtteranceId(2)]);
        assert_eq!(item.transcript.timing.end, StreamTime::from_seconds(2.0));
        assert!(matches!(
            &batch.events[0],
            PipelineEvent::UnitSealed { ids, .. } if *ids == vec![UtteranceId(1), UtteranceId(2)]
        ));
        assert_eq!(
            queue
                .take(StreamTime::from_seconds(3.0))
                .item
                .unwrap()
                .transcript
                .id,
            UtteranceId(3)
        );

        // Lines on either side of a break, or of a gap of more than 10 s, are never translated
        // as one sentence.
        let mut queue = TranslationQueue::default();
        queue.push(transcript(1, "你好。", 1.0), 0).unwrap();
        queue.discontinuity(StreamTime::from_seconds(1.5));
        queue.push(transcript(2, "再见。", 3.0), 0).unwrap();
        queue.push(transcript(3, "走吧。", 15.0), 0).unwrap();
        // Taken at each line's own end, so the skip rule stays out of it.
        for (id, end) in [(1, 1.0), (2, 3.0), (3, 15.0)] {
            let batch = queue.take(StreamTime::from_seconds(end));
            assert!(batch.events.is_empty());
            assert_eq!(batch.item.unwrap().transcript.id, UtteranceId(id));
        }

        // Two breaks that arrive before the line between them still separate both sides: the
        // first break is not consumed by the line it precedes while the second is still queued.
        let mut queue = TranslationQueue::default();
        queue.discontinuity(StreamTime::from_seconds(23.5));
        queue.discontinuity(StreamTime::from_seconds(25.5));
        queue.push(transcript(1, "你好。", 25.0), 0).unwrap();
        queue.push(transcript(2, "再见。", 27.0), 0).unwrap();
        for (id, end) in [(1, 25.0), (2, 27.0)] {
            let batch = queue.take(StreamTime::from_seconds(end));
            assert_eq!(batch.item.unwrap().transcript.id, UtteranceId(id));
        }
    }

    #[test]
    fn skip_at_more_than_six_seconds_only_and_keep_newest() {
        let mut queue = TranslationQueue::default();
        queue.push(transcript(1, "你好。", 1.0), 1000).unwrap();
        queue.push(transcript(2, "再见。", 2.0), 2000).unwrap();
        let batch = queue.take(StreamTime::from_seconds(7.5));
        assert_eq!(batch.skipped, 1);
        assert!(matches!(
            batch.events[0],
            PipelineEvent::Skipped {
                id: UtteranceId(1),
                ..
            }
        ));
        assert_eq!(batch.item.unwrap().transcript.id, UtteranceId(2));
        for live in [6.9, 7.0] {
            let mut queue = TranslationQueue::default();
            queue.push(transcript(1, "你好。", 1.0), 1000).unwrap();
            queue.push(transcript(2, "再见。", 2.0), 2000).unwrap();
            assert!(queue.take(StreamTime::from_seconds(live)).skipped == 0);
        }
    }
}
