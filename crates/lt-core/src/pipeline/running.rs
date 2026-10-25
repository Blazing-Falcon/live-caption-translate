//! Running text, also used by the caption panel: closed lines are translated together up to a
//! sentence end, and the part not yet translated for good is translated again whenever the
//! translator is free, so English follows the speech instead of waiting for each line to close.

use crate::{
    text::classify,
    types::{CutReason, StageTiming, TextClass, Transcript, UtteranceId},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// Closed lines translated as one unit, at most. A whole sentence translates better than its
/// comma clauses; a longer unit keeps speech without sentence ends untranslated for good for
/// many seconds and makes every preview slower.
const UNIT_CHARS: usize = 40;
/// After a silence this long the next line starts a new unit, as it starts a new window.
const UNIT_GAP_S: f64 = 10.0;
const SENTENCE_ENDS: &[char] = &['。', '？', '！', '?', '!'];
/// Least time between the end of one preview and the start of the next, so a preview does not
/// run for every new word. The wait is at least as long as the last preview took, so previews
/// use at most about half of the translator: fast speech without sentence ends otherwise kept it
/// busy most of the time.
const PREVIEW_GAP: Duration = Duration::from_millis(300);
/// With fewer letters the model explains the word instead of translating it.
const PREVIEW_MIN_CHARS: usize = 4;

#[derive(Default)]
pub(super) struct RunningText {
    /// Closed lines not yet in a unit, oldest first.
    active: Vec<Transcript>,
    /// Live text of the lines that have no final text yet: the open line, and a closed line
    /// whose final waits for the second pass while the next line is already heard.
    heard: BTreeMap<UtteranceId, String>,
    /// The text of the last preview sent.
    previewed: String,
    preview_started: Option<Instant>,
    next_preview: Option<Instant>,
}

impl RunningText {
    /// A closed line to translate. Returns the units to translate for good, oldest first.
    pub(super) fn push(&mut self, transcript: Transcript) -> Vec<Transcript> {
        let mut units = Vec::new();
        if let Some(last) = self.active.last() {
            let used: usize = self
                .active
                .iter()
                .map(|line| line.text.chars().count())
                .sum();
            let gap = transcript.timing.start.saturating_sub(last.timing.end);
            if used + transcript.text.chars().count() > UNIT_CHARS || gap.seconds() > UNIT_GAP_S {
                units.extend(self.freeze());
            }
        }
        let ends = transcript.cut != CutReason::Commit
            || transcript.text.trim_end().ends_with(SENTENCE_ENDS);
        self.active.push(transcript);
        if ends {
            units.extend(self.freeze());
        }
        units
    }

    /// Every closed line as one unit.
    pub(super) fn freeze(&mut self) -> Option<Transcript> {
        let mut lines = std::mem::take(&mut self.active).into_iter();
        let mut unit = lines.next()?;
        for line in lines {
            unit.absorbed.push(line.id);
            unit.absorbed.extend(line.absorbed);
            unit.text.push_str(&line.text);
            unit.timing.end = line.timing.end;
            unit.timing.asr_done_ms = unit.timing.asr_done_ms.max(line.timing.asr_done_ms);
            unit.timing.asr_ms = unit.timing.asr_ms.saturating_add(line.timing.asr_ms);
            unit.cut = line.cut;
            if line.class == TextClass::Mixed {
                unit.class = TextClass::Mixed;
            }
        }
        Some(unit)
    }

    pub(super) fn partial(&mut self, id: UtteranceId, text: String) {
        self.heard.insert(id, text);
    }

    /// The line got its final text or was dropped.
    pub(super) fn closed(&mut self, id: UtteranceId) {
        self.heard.remove(&id);
    }

    /// The text to translate again now, as a line led by the first id it covers, with every id
    /// it covers. `None` while the text is unchanged, too short or the last preview too recent.
    pub(super) fn preview(&mut self, now: Instant) -> Option<(Transcript, Vec<UtteranceId>)> {
        if self.next_preview.is_some_and(|next| now < next) {
            return None;
        }
        let mut ids: Vec<UtteranceId> = self.active.iter().map(|line| line.id).collect();
        let mut text: String = self.active.iter().map(|line| line.text.as_str()).collect();
        for (id, partial) in &self.heard {
            ids.push(*id);
            text.push_str(partial);
        }
        let letters = text.chars().filter(|c| c.is_alphanumeric()).count();
        if letters < PREVIEW_MIN_CHARS || text == self.previewed {
            return None;
        }
        let class = classify(&text)?;
        self.previewed.clone_from(&text);
        self.preview_started = Some(now);
        let first = self.active.first();
        let transcript = Transcript {
            id: ids[0],
            text,
            class,
            timing: StageTiming {
                start: first.map_or_else(Default::default, |line| line.timing.start),
                end: first.map_or_else(Default::default, |line| line.timing.end),
                asr_done_ms: 0,
                asr_ms: 0,
            },
            absorbed: Vec::new(),
            cut: CutReason::Commit,
            logprob: None,
            second_pass: crate::types::SecondPass::default(),
        };
        Some((transcript, ids))
    }

    pub(super) fn preview_done(&mut self, now: Instant) {
        let took = self
            .preview_started
            .map_or(Duration::ZERO, |started| now.duration_since(started));
        self.next_preview = Some(now + took.max(PREVIEW_GAP));
    }

    /// Whether a finished preview still covers only lines not translated for good: a line in
    /// it may have joined a unit or been dropped while it ran.
    pub(super) fn current(&self, ids: &[UtteranceId]) -> bool {
        ids.iter()
            .all(|id| self.active.iter().any(|line| line.id == *id) || self.heard.contains_key(id))
    }
}
