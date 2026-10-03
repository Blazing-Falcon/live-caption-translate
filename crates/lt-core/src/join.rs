//! Hold short phrases until a timely successor's ASR can complete.
use crate::clock::Clock;
use crate::config::JoinConfig;
use crate::events::{JoinKind, PipelineEvent};
use crate::text::chinese_chars;
use crate::types::{StreamTime, TextClass, Transcript, UtteranceId};
use std::collections::VecDeque;
use std::sync::Arc;

// Four queued ASR segments, the active segment, and up to three held segments.
const MAX_START_NOTICES: usize = 8;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoinResult {
    pub ready: Vec<Transcript>,
    pub events: Vec<PipelineEvent>,
}

struct Held {
    transcript: Transcript,
    tail: UtteranceId,
    segments: u32,
}

#[derive(Clone, Copy)]
struct StartNotice {
    id: UtteranceId,
    at: Option<StreamTime>,
    dropped: bool,
}

pub struct Joiner {
    config: JoinConfig,
    clock: Arc<dyn Clock>,
    held: Option<Held>,
    starts: VecDeque<StartNotice>,
    processed_through: Option<UtteranceId>,
    discontinuity_at: Option<StreamTime>,
}

impl Joiner {
    pub fn new(config: JoinConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            config,
            clock,
            held: None,
            starts: VecDeque::with_capacity(MAX_START_NOTICES),
            processed_through: None,
            discontinuity_at: None,
        }
    }

    pub fn held_count(&self) -> u32 {
        self.held.as_ref().map_or(0, |held| held.segments)
    }

    pub fn push(&mut self, transcript: Transcript) -> JoinResult {
        let mut result = JoinResult::default();
        let incoming_id = transcript.id;
        let speech_start = self
            .starts
            .iter()
            .find(|notice| notice.id == incoming_id)
            .and_then(|notice| notice.at)
            .unwrap_or(transcript.timing.start);
        let before_discontinuity = self
            .discontinuity_at
            .is_some_and(|at| transcript.timing.end <= at);
        let mut incoming = Held {
            segments: segment_count(&transcript),
            tail: incoming_id,
            transcript,
        };
        if let Some(mut held) = self.held.take() {
            let timely = speech_start
                .saturating_sub(held.transcript.timing.end)
                .samples()
                <= self.window_samples();
            let skipped_notice = self
                .next_notice(held.tail)
                .is_some_and(|notice| notice.id < incoming_id || notice.dropped);
            let total_chars = chinese_chars(&held.transcript.text)
                .saturating_add(chinese_chars(&incoming.transcript.text));
            let total_segments = held.segments.saturating_add(incoming.segments);
            let can_join = incoming_id > held.tail
                && !before_discontinuity
                && !skipped_notice
                && timely
                && joinable(incoming.transcript.class)
                && total_chars <= self.config.max_chars as usize
                && total_segments <= self.config.max_segments;
            if can_join {
                let absorbed = merge_transcript(&mut held.transcript, incoming.transcript);
                held.tail = incoming_id;
                held.segments = total_segments;
                result.events.push(PipelineEvent::Joined {
                    id: held.transcript.id,
                    absorbed,
                    text: held.transcript.text.clone(),
                    kind: JoinKind::Hold,
                });
                incoming = held;
            } else {
                result.ready.push(held.transcript);
            }
        }
        if !before_discontinuity && self.should_hold(&incoming) {
            self.held = Some(incoming);
        } else {
            result.ready.push(incoming.transcript);
        }
        self.mark_processed(incoming_id);
        self.starts.retain(|notice| notice.id > incoming_id);
        self.release_if_due(&mut result);
        result
    }

    /// Remember starts even while the previous transcript is still in ASR.
    pub fn speech_started(&mut self, id: UtteranceId, at: StreamTime) -> JoinResult {
        let mut result = JoinResult::default();
        if self
            .processed_through
            .is_some_and(|processed| id <= processed)
            || self.discontinuity_at.is_some_and(|boundary| at < boundary)
        {
            return result;
        }
        if let Some(notice) = self.starts.iter_mut().find(|notice| notice.id == id) {
            notice.at = Some(at);
        } else {
            self.insert_notice(
                StartNotice {
                    id,
                    at: Some(at),
                    dropped: false,
                },
                &mut result,
            );
        }
        self.release_if_due(&mut result);
        result
    }

    pub fn tick(&mut self) -> JoinResult {
        let mut result = JoinResult::default();
        self.release_if_due(&mut result);
        result
    }

    pub fn discontinuity(&mut self) -> JoinResult {
        self.discontinuity_at(self.clock.now())
    }

    pub fn discontinuity_at(&mut self, at: StreamTime) -> JoinResult {
        let result = self.finish();
        self.discontinuity_at = Some(at);
        result
    }

    pub fn finish(&mut self) -> JoinResult {
        let mut result = JoinResult::default();
        self.release(&mut result);
        self.starts.clear();
        result
    }

    /// A filtered successor cannot provide a transcript to satisfy the hold.
    pub fn dropped(&mut self, id: UtteranceId) -> JoinResult {
        let mut result = JoinResult::default();
        if self
            .held
            .as_ref()
            .is_some_and(|held| held.transcript.id == id || held.transcript.absorbed.contains(&id))
        {
            self.held = None;
        }
        if let Some(notice) = self.starts.iter_mut().find(|notice| notice.id == id) {
            notice.dropped = true;
        } else {
            self.insert_notice(
                StartNotice {
                    id,
                    at: None,
                    dropped: true,
                },
                &mut result,
            );
        }
        self.mark_processed(id);
        self.release_if_due(&mut result);
        result
    }

    fn should_hold(&self, held: &Held) -> bool {
        let count = chinese_chars(&held.transcript.text);
        self.config.hold_max_chars != 0
            && joinable(held.transcript.class)
            && count <= self.config.hold_max_chars as usize
            && count < self.config.max_chars as usize
            && held.segments < self.config.max_segments
    }

    fn window_samples(&self) -> u64 {
        StreamTime::from_seconds(f64::from(self.config.hold_window_s)).samples()
    }

    fn next_notice(&self, tail: UtteranceId) -> Option<StartNotice> {
        self.starts
            .iter()
            .filter(|notice| notice.id > tail)
            .min_by_key(|notice| notice.id)
            .copied()
    }

    fn release_if_due(&mut self, result: &mut JoinResult) {
        if let Some(held) = &self.held {
            let release = match self.next_notice(held.tail) {
                Some(notice) if notice.dropped => true,
                Some(notice) => notice.at.is_some_and(|at| {
                    at.saturating_sub(held.transcript.timing.end).samples() > self.window_samples()
                }),
                None => {
                    self.clock
                        .now()
                        .saturating_sub(held.transcript.timing.end)
                        .samples()
                        > self.window_samples()
                }
            };
            if release {
                self.release(result);
            }
        }
    }

    fn release(&mut self, result: &mut JoinResult) {
        if let Some(held) = self.held.take() {
            result.ready.push(held.transcript);
        }
    }

    fn mark_processed(&mut self, id: UtteranceId) {
        self.processed_through = Some(
            self.processed_through
                .map_or(id, |previous| previous.max(id)),
        );
    }

    fn insert_notice(&mut self, notice: StartNotice, result: &mut JoinResult) {
        if self.starts.len() == MAX_START_NOTICES {
            let awaited = self
                .held
                .as_ref()
                .and_then(|held| self.next_notice(held.tail))
                .map(|notice| notice.id);
            if self
                .starts
                .pop_front()
                .is_some_and(|evicted| awaited == Some(evicted.id))
            {
                self.release(result);
            }
        }
        self.starts.push_back(notice);
    }
}

fn joinable(class: TextClass) -> bool {
    matches!(class, TextClass::Chinese | TextClass::Mixed)
}

fn segment_count(transcript: &Transcript) -> u32 {
    match u32::try_from(transcript.absorbed.len()) {
        Ok(absorbed) => absorbed.saturating_add(1),
        Err(_) => u32::MAX,
    }
}

/// SenseVoice sentence punctuation becomes a comma at the join boundary.
pub fn join_text(first: &str, next: &str) -> String {
    let first = first.trim_end();
    let mut joined = match first.strip_suffix('。').or_else(|| first.strip_suffix('.')) {
        Some(prefix) => format!("{prefix}，"),
        None => first.to_owned(),
    };
    joined.push_str(next.trim_start());
    joined
}

fn merge_transcript(leader: &mut Transcript, incoming: Transcript) -> Vec<UtteranceId> {
    leader.text = join_text(&leader.text, &incoming.text);
    leader.timing.end = leader.timing.end.max(incoming.timing.end);
    leader.timing.asr_done_ms = leader.timing.asr_done_ms.max(incoming.timing.asr_done_ms);
    leader.timing.asr_ms = leader.timing.asr_ms.saturating_add(incoming.timing.asr_ms);
    if incoming.class == TextClass::Mixed {
        leader.class = TextClass::Mixed;
    }
    let newly_absorbed = if incoming.id != leader.id && !leader.absorbed.contains(&incoming.id) {
        vec![incoming.id]
    } else {
        Vec::new()
    };
    for id in std::iter::once(incoming.id).chain(incoming.absorbed) {
        if id != leader.id && !leader.absorbed.contains(&id) {
            leader.absorbed.push(id);
        }
    }
    newly_absorbed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::types::StageTiming;

    fn time(seconds: f64) -> StreamTime {
        StreamTime::from_seconds(seconds)
    }

    fn transcript(id: u64, text: &str, class: TextClass, start: f64, end: f64) -> Transcript {
        Transcript {
            id: UtteranceId(id),
            text: text.into(),
            lang_tag: Some("zh".into()),
            class,
            event: None,
            timing: StageTiming {
                start: time(start),
                end: time(end),
                asr_done_ms: time(end).millis() + 300,
                asr_ms: 300,
            },
            absorbed: Vec::new(),
        }
    }

    fn setup(config: JoinConfig, seconds: f64) -> (ManualClock, Joiner) {
        let clock = ManualClock::new(time(seconds));
        let joiner = Joiner::new(config, Arc::new(clock.clone()));
        (clock, joiner)
    }

    #[test]
    fn keynote_fragments_join_once_with_leader_and_complete_timing() {
        let (clock, mut joiner) = setup(JoinConfig::default(), 10.0);
        assert!(joiner
            .push(transcript(
                1,
                "我也想办一个。",
                TextClass::Chinese,
                9.0,
                10.0
            ))
            .ready
            .is_empty());
        assert_eq!(joiner.held_count(), 1);
        assert!(joiner
            .speech_started(UtteranceId(2), time(10.6))
            .ready
            .is_empty());
        clock.set(time(13.0));
        assert!(
            joiner.tick().ready.is_empty(),
            "timely speech waits for its ASR"
        );
        let result = joiner.push(transcript(
            2,
            "伟大的公司。",
            TextClass::Chinese,
            10.3,
            12.0,
        ));
        assert_eq!(result.ready.len(), 1);
        let ready = &result.ready[0];
        assert_eq!(ready.id, UtteranceId(1));
        assert_eq!(ready.text, "我也想办一个，伟大的公司。");
        assert_eq!(ready.absorbed, vec![UtteranceId(2)]);
        assert_eq!(ready.timing.start, time(9.0));
        assert_eq!(ready.timing.end, time(12.0));
        assert_eq!(ready.timing.asr_done_ms, 12_300);
        assert_eq!(ready.timing.asr_ms, 600);
        assert_eq!(
            result.events,
            vec![PipelineEvent::Joined {
                id: UtteranceId(1),
                absorbed: vec![UtteranceId(2)],
                text: "我也想办一个，伟大的公司。".into(),
                kind: JoinKind::Hold,
            }]
        );
        assert_eq!(joiner.held_count(), 0);
        assert!(joiner.finish().ready.is_empty());
    }

    #[test]
    fn timeout_has_sample_exact_boundary_and_late_speech_does_not_join() {
        let (clock, mut joiner) = setup(JoinConfig::default(), 10.0);
        joiner.push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0));
        clock.set(time(11.0));
        assert!(joiner.tick().ready.is_empty());
        clock.advance(StreamTime::from_samples(1));
        assert_eq!(joiner.tick().ready[0].id, UtteranceId(1));
        clock.set(time(11.2));
        assert!(joiner
            .speech_started(UtteranceId(2), time(11.2))
            .ready
            .is_empty());
        assert!(joiner
            .push(transcript(2, "今天。", TextClass::Chinese, 11.0, 12.0))
            .events
            .is_empty());
        assert_eq!(joiner.finish().ready[0].id, UtteranceId(2));
    }

    #[test]
    fn speech_at_window_boundary_waits_for_asr() {
        let (clock, mut joiner) = setup(JoinConfig::default(), 10.0);
        joiner.push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0));
        joiner.speech_started(UtteranceId(2), time(11.0));
        clock.set(time(20.0));
        assert!(joiner.tick().ready.is_empty());
        let result = joiner.push(transcript(
            2,
            "今天谈一些事情。",
            TextClass::Chinese,
            10.7,
            12.0,
        ));
        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].text, "我们，今天谈一些事情。");
    }

    #[test]
    fn next_start_known_before_leader_asr_prevents_premature_timeout() {
        let (clock, mut joiner) = setup(JoinConfig::default(), 13.0);
        joiner.speech_started(UtteranceId(1), time(9.0));
        joiner.speech_started(UtteranceId(2), time(10.6));
        joiner.speech_started(UtteranceId(3), time(13.0));
        assert!(joiner
            .push(transcript(
                1,
                "我也想办一个。",
                TextClass::Chinese,
                8.7,
                10.0
            ))
            .ready
            .is_empty());
        clock.set(time(14.0));
        assert!(joiner.tick().ready.is_empty());
        let result = joiner.push(transcript(
            2,
            "伟大的公司。",
            TextClass::Chinese,
            10.3,
            12.0,
        ));
        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].text, "我也想办一个，伟大的公司。");
    }

    #[test]
    fn late_recorded_start_overrides_earlier_pre_roll_position() {
        let (_, mut joiner) = setup(JoinConfig::default(), 10.0);
        joiner.push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0));
        let released = joiner.speech_started(UtteranceId(2), time(11.2));
        assert_eq!(released.ready[0].id, UtteranceId(1));
        let result = joiner.push(transcript(2, "今天。", TextClass::Chinese, 10.9, 12.0));
        assert!(result.events.is_empty());
        assert_eq!(joiner.finish().ready[0].text, "今天。");
    }

    #[test]
    fn transcript_start_is_the_fallback_when_speech_notices_are_unavailable() {
        let (clock, mut joiner) = setup(JoinConfig::default(), 10.0);
        joiner.push(transcript(
            1,
            "我也想办一个。",
            TextClass::Chinese,
            9.0,
            10.0,
        ));
        clock.set(time(13.0));
        let result = joiner.push(transcript(
            2,
            "伟大的公司。",
            TextClass::Chinese,
            10.6,
            12.0,
        ));
        assert_eq!(result.ready[0].text, "我也想办一个，伟大的公司。");
    }

    #[test]
    fn third_short_phrase_hits_segment_limit_and_fourth_starts_new_hold() {
        let (clock, mut joiner) = setup(JoinConfig::default(), 1.0);
        joiner.push(transcript(1, "我们。", TextClass::Chinese, 0.0, 1.0));
        clock.set(time(2.0));
        let second = joiner.push(transcript(2, "今天。", TextClass::Chinese, 1.5, 2.0));
        assert!(second.ready.is_empty());
        assert_eq!(joiner.held_count(), 2);
        clock.set(time(3.0));
        let third = joiner.push(transcript(3, "一起。", TextClass::Chinese, 2.5, 3.0));
        assert_eq!(third.ready[0].text, "我们，今天，一起。");
        assert_eq!(
            third.ready[0].absorbed,
            vec![UtteranceId(2), UtteranceId(3)]
        );
        assert_eq!(
            third.events[0],
            PipelineEvent::Joined {
                id: UtteranceId(1),
                absorbed: vec![UtteranceId(3)],
                text: "我们，今天，一起。".into(),
                kind: JoinKind::Hold,
            }
        );
        assert_eq!(joiner.held_count(), 0);
        clock.set(time(4.0));
        assert!(joiner
            .push(transcript(4, "说说。", TextClass::Chinese, 3.5, 4.0))
            .ready
            .is_empty());
        assert_eq!(joiner.finish().ready[0].id, UtteranceId(4));
    }

    #[test]
    fn chinese_character_cap_is_checked_before_merging() {
        let (_, mut joiner) = setup(JoinConfig::default(), 10.0);
        joiner.push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0));
        let long = format!("{}。", "中".repeat(39));
        let result = joiner.push(transcript(2, &long, TextClass::Chinese, 10.5, 12.0));
        assert_eq!(result.ready.len(), 2);
        assert_eq!(result.ready[0].text, "我们。");
        assert_eq!(result.ready[1].text, long);
        assert!(result.events.is_empty());
    }

    #[test]
    fn exact_character_cap_joins_and_releases() {
        let config = JoinConfig {
            hold_max_chars: 20,
            max_chars: 10,
            ..JoinConfig::default()
        };
        let (_, mut joiner) = setup(config, 10.0);
        joiner.push(transcript(
            1,
            "我们今天一起。",
            TextClass::Chinese,
            9.0,
            10.0,
        ));
        let result = joiner.push(transcript(2, "谈谈天气。", TextClass::Chinese, 10.5, 12.0));
        assert_eq!(result.ready[0].text, "我们今天一起，谈谈天气。");
        assert_eq!(joiner.held_count(), 0);
    }

    #[test]
    fn mixed_class_counts_only_ideographs_and_keeps_all_languages() {
        let (clock, mut joiner) = setup(JoinConfig::default(), 10.0);
        assert!(joiner
            .push(transcript(
                1,
                "我们 discuss privacy。",
                TextClass::Mixed,
                9.0,
                10.0
            ))
            .ready
            .is_empty());
        clock.set(time(12.0));
        let result = joiner.push(transcript(2, "一起。", TextClass::Chinese, 10.5, 12.0));
        assert!(result.ready.is_empty());
        let ready = joiner.finish().ready.remove(0);
        assert_eq!(ready.text, "我们 discuss privacy，一起。");
        assert_eq!(ready.class, TextClass::Mixed);
    }

    #[test]
    fn english_and_other_flush_without_joining() {
        for (class, text) in [
            (TextClass::English, "Hello world."),
            (TextClass::Other, "こんにちは。"),
        ] {
            let (_, mut joiner) = setup(JoinConfig::default(), 10.0);
            joiner.push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0));
            let result = joiner.push(transcript(2, text, class, 10.5, 12.0));
            assert_eq!(result.ready.len(), 2);
            assert_eq!(result.ready[0].text, "我们。");
            assert_eq!(result.ready[1].text, text);
            assert!(result.events.is_empty());
            assert_eq!(joiner.held_count(), 0);
        }
    }

    #[test]
    fn disabled_joining_passes_every_transcript_immediately() {
        let config = JoinConfig {
            hold_max_chars: 0,
            ..JoinConfig::default()
        };
        let (_, mut joiner) = setup(config, 10.0);
        assert_eq!(
            joiner
                .push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0))
                .ready
                .len(),
            1
        );
        assert_eq!(joiner.held_count(), 0);
    }

    #[test]
    fn discontinuity_flushes_and_late_old_asr_cannot_cross_the_fence() {
        let (clock, mut joiner) = setup(JoinConfig::default(), 10.0);
        joiner.push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0));
        clock.set(time(12.0));
        assert_eq!(joiner.discontinuity().ready[0].id, UtteranceId(1));
        assert_eq!(
            joiner
                .push(transcript(2, "过去。", TextClass::Chinese, 10.5, 11.5))
                .ready[0]
                .id,
            UtteranceId(2)
        );
        clock.set(time(14.0));
        assert!(joiner
            .push(transcript(3, "现在。", TextClass::Chinese, 12.5, 14.0))
            .ready
            .is_empty());
        assert_eq!(joiner.finish().ready[0].text, "现在。");
    }

    #[test]
    fn dropped_successor_unblocks_hold_even_if_drop_precedes_leader_asr() {
        for early_drop in [false, true] {
            let (clock, mut joiner) = setup(JoinConfig::default(), 10.0);
            joiner.speech_started(UtteranceId(1), time(9.0));
            joiner.speech_started(UtteranceId(2), time(10.5));
            if early_drop {
                joiner.dropped(UtteranceId(2));
                assert_eq!(
                    joiner
                        .push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0))
                        .ready[0]
                        .id,
                    UtteranceId(1)
                );
            } else {
                joiner.push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0));
                clock.set(time(20.0));
                assert!(joiner.tick().ready.is_empty());
                assert_eq!(joiner.dropped(UtteranceId(2)).ready[0].id, UtteranceId(1));
            }
            assert_eq!(joiner.held_count(), 0);
        }
    }

    #[test]
    fn pending_start_history_is_bounded_and_eviction_releases_awaited_hold() {
        let (_, mut joiner) = setup(JoinConfig::default(), 10.0);
        joiner.push(transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0));
        let mut released = Vec::new();
        for id in 2..=20 {
            released.extend(joiner.speech_started(UtteranceId(id), time(10.5)).ready);
            assert!(joiner.starts.len() <= MAX_START_NOTICES);
        }
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].id, UtteranceId(1));
        assert_eq!(joiner.held_count(), 0);
    }

    #[test]
    fn joined_absorbed_ids_are_new_only_and_asr_duration_saturates() {
        let (_, mut joiner) = setup(JoinConfig::default(), 10.0);
        let mut first = transcript(1, "我们。", TextClass::Chinese, 9.0, 10.0);
        first.timing.asr_ms = u32::MAX;
        joiner.push(first);
        let mut next = transcript(2, "今天。", TextClass::Mixed, 10.5, 12.0);
        next.absorbed = vec![UtteranceId(3)];
        let result = joiner.push(next);
        assert_eq!(
            result.ready[0].absorbed,
            vec![UtteranceId(2), UtteranceId(3)]
        );
        assert_eq!(result.ready[0].timing.asr_ms, u32::MAX);
        assert_eq!(result.ready[0].class, TextClass::Mixed);
        assert_eq!(
            result.events[0],
            PipelineEvent::Joined {
                id: UtteranceId(1),
                absorbed: vec![UtteranceId(2)],
                text: "我们，今天。".into(),
                kind: JoinKind::Hold,
            }
        );
    }

    #[test]
    fn join_text_handles_unicode_sentence_marks_and_preserves_reduplication() {
        assert_eq!(join_text("谢谢。 ", " 看看。"), "谢谢，看看。");
        assert_eq!(join_text("我们.", "今天。"), "我们，今天。");
        assert_eq!(join_text("我们！", "今天。"), "我们！今天。");
        assert_eq!(join_text("我们", "今天。"), "我们今天。");
    }
}
