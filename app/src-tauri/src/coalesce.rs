//! Bridge throttle: newest `text_so_far` per id, at most one delta per id every
//! 33 ms, everything else immediate and in order.
//!
//! Ordering policy: a pending delta happened before any later event, so every non-delta event
//! first flushes all older pending deltas (in arrival order) and only then is forwarded. The one
//! exception is a terminal event for the same id, whose own pending delta is discarded.
use lt_core::events::PipelineEvent;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub const DELTA_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Default)]
pub struct Coalescer {
    /// Ids in the order their first pending delta arrived.
    order: Vec<u64>,
    pending: HashMap<u64, String>,
    last_sent: HashMap<u64, Instant>,
}

impl Coalescer {
    pub fn push(&mut self, event: PipelineEvent, now: Instant) -> Vec<PipelineEvent> {
        let mut out = Vec::new();
        match event {
            PipelineEvent::TranslationDelta { id, text_so_far } => {
                let id = id.0;
                if self.pending.contains_key(&id) {
                    self.pending.insert(id, text_so_far);
                } else if self
                    .last_sent
                    .get(&id)
                    .is_some_and(|sent| now.duration_since(*sent) < DELTA_INTERVAL)
                {
                    self.order.push(id);
                    self.pending.insert(id, text_so_far);
                } else {
                    // Older pending deltas of other ids must not be overtaken.
                    self.flush_all(now, &mut out);
                    self.last_sent.insert(id, now);
                    out.push(delta(id, text_so_far));
                }
            }
            other => {
                if let PipelineEvent::TranslationFinal { id, .. }
                | PipelineEvent::TranslationFailed { id, .. }
                | PipelineEvent::Skipped { id, .. } = &other
                {
                    self.discard(id.0);
                    self.last_sent.remove(&id.0);
                } else if let PipelineEvent::Joined { absorbed, .. } = &other {
                    for id in absorbed {
                        self.discard(id.0);
                        self.last_sent.remove(&id.0);
                    }
                }
                self.flush_all(now, &mut out);
                out.push(other);
            }
        }
        out
    }

    /// Pending deltas whose interval has elapsed, oldest first.
    pub fn flush_due(&mut self, now: Instant) -> Vec<PipelineEvent> {
        let mut out = Vec::new();
        let due = |this: &Self, id: &u64| {
            this.last_sent
                .get(id)
                .is_none_or(|sent| now.duration_since(*sent) >= DELTA_INTERVAL)
        };
        // A later id never overtakes an earlier pending one.
        while let Some(&id) = self.order.first() {
            if !due(self, &id) {
                break;
            }
            self.order.remove(0);
            if let Some(text) = self.pending.remove(&id) {
                self.last_sent.insert(id, now);
                out.push(delta(id, text));
            }
        }
        out
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let id = self.order.first()?;
        Some(
            self.last_sent
                .get(id)
                .map_or_else(Instant::now, |sent| *sent + DELTA_INTERVAL),
        )
    }

    #[cfg(test)]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    fn discard(&mut self, id: u64) {
        if self.pending.remove(&id).is_some() {
            self.order.retain(|queued| *queued != id);
        }
    }

    fn flush_all(&mut self, now: Instant, out: &mut Vec<PipelineEvent>) {
        for id in std::mem::take(&mut self.order) {
            if let Some(text) = self.pending.remove(&id) {
                self.last_sent.insert(id, now);
                out.push(delta(id, text));
            }
        }
        // Idle ids would otherwise grow the map for a whole session.
        if self.last_sent.len() > 256 {
            self.last_sent
                .retain(|_, sent| now.duration_since(*sent) < DELTA_INTERVAL);
        }
    }
}

fn delta(id: u64, text_so_far: String) -> PipelineEvent {
    PipelineEvent::TranslationDelta {
        id: lt_core::types::UtteranceId(id),
        text_so_far,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::{
        events::{JoinKind, SkipReason},
        types::{Timing, UtteranceId},
    };

    fn d(id: u64, text: &str) -> PipelineEvent {
        delta(id, text.into())
    }
    fn texts(events: &[PipelineEvent]) -> Vec<String> {
        events
            .iter()
            .map(|event| match event {
                PipelineEvent::TranslationDelta { id, text_so_far } => {
                    format!("d{}:{text_so_far}", id.0)
                }
                PipelineEvent::TranslationFinal { id, .. } => format!("f{}", id.0),
                PipelineEvent::AsrFinal { id, .. } => format!("a{}", id.0),
                PipelineEvent::Skipped { id, .. } => format!("s{}", id.0),
                PipelineEvent::Joined { id, .. } => format!("j{}", id.0),
                _ => "other".into(),
            })
            .collect()
    }
    fn fin(id: u64) -> PipelineEvent {
        PipelineEvent::TranslationFinal {
            id: UtteranceId(id),
            text: "x".into(),
            timing: Timing::default(),
        }
    }
    fn asr(id: u64) -> PipelineEvent {
        PipelineEvent::AsrFinal {
            id: UtteranceId(id),
            text: "你好".into(),
            class: lt_core::types::TextClass::Chinese,
            lang: None,
            start_ms: 0,
            end_ms: 1,
            asr_ms: 1,
        }
    }

    #[test]
    fn first_delta_is_immediate_and_rapid_followers_keep_only_the_newest() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        assert_eq!(texts(&c.push(d(1, "a"), t0)), ["d1:a"]);
        assert!(c.push(d(1, "ab"), t0 + Duration::from_millis(5)).is_empty());
        assert!(c
            .push(d(1, "abc"), t0 + Duration::from_millis(10))
            .is_empty());
        assert_eq!(c.pending_len(), 1);
        assert!(c.flush_due(t0 + Duration::from_millis(20)).is_empty());
        assert_eq!(
            texts(&c.flush_due(t0 + Duration::from_millis(34))),
            ["d1:abc"]
        );
        assert_eq!(c.pending_len(), 0);
        assert!(c.next_deadline().is_none());
    }

    #[test]
    fn final_discards_its_pending_delta_but_flushes_older_ones_first() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.push(d(1, "a"), t0);
        c.push(d(2, "b"), t0);
        c.push(d(2, "b2"), t0 + Duration::from_millis(1));
        c.push(d(1, "a2"), t0 + Duration::from_millis(2));
        // Pending order is [2, 1]; the final for 1 drops delta 1 but keeps delta 2 ahead of it.
        let out = c.push(fin(1), t0 + Duration::from_millis(3));
        assert_eq!(texts(&out), ["d2:b2", "f1"]);
        assert_eq!(c.pending_len(), 0);
    }

    #[test]
    fn cross_id_order_and_other_events_never_overtake_pending_deltas() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.push(d(1, "a"), t0);
        c.push(d(1, "a2"), t0 + Duration::from_millis(1));
        let out = c.push(asr(2), t0 + Duration::from_millis(2));
        assert_eq!(texts(&out), ["d1:a2", "a2"]);
        // A new delta for another id flushes older pending ones first.
        c.push(d(3, "x"), t0 + Duration::from_millis(3));
        c.push(d(3, "x2"), t0 + Duration::from_millis(4));
        let out = c.push(d(4, "y"), t0 + Duration::from_millis(5));
        assert_eq!(texts(&out), ["d3:x2", "d4:y"]);
    }

    #[test]
    fn skipped_and_absorbed_ids_drop_pending_deltas() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.push(d(5, "a"), t0);
        c.push(d(5, "b"), t0 + Duration::from_millis(1));
        let out = c.push(
            PipelineEvent::Skipped {
                id: UtteranceId(5),
                reason: SkipReason::CatchUp,
            },
            t0 + Duration::from_millis(2),
        );
        assert_eq!(texts(&out), ["s5"]);
        c.push(d(6, "a"), t0);
        c.push(d(6, "b"), t0 + Duration::from_millis(1));
        let out = c.push(
            PipelineEvent::Joined {
                id: UtteranceId(7),
                absorbed: vec![UtteranceId(6)],
                text: "t".into(),
                kind: JoinKind::Queue,
            },
            t0 + Duration::from_millis(2),
        );
        assert_eq!(texts(&out), ["j7"]);
    }

    #[test]
    fn deadline_tracks_the_oldest_pending_delta_and_memory_stays_bounded() {
        let t0 = Instant::now();
        let mut c = Coalescer::default();
        c.push(d(1, "a"), t0);
        c.push(d(1, "b"), t0 + Duration::from_millis(1));
        assert_eq!(c.next_deadline(), Some(t0 + DELTA_INTERVAL));
        for id in 100..2_000u64 {
            let now = t0 + Duration::from_secs(id);
            c.push(d(id, "z"), now);
            c.push(fin(id), now);
        }
        assert!(c.last_sent.len() <= 260);
    }
}
