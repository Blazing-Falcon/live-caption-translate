use lt_core::{
    events::PipelineEvent,
    pipeline::ClauseCounts,
    types::{SecondPass, SecondPassOutcome, TextClass, UtteranceId},
};
use serde::Serialize;
use std::collections::BTreeMap;

/// Complete-session latency samples, distinct from the UI's last-50 window.
pub struct ReplayReport {
    status: BTreeMap<&'static str, u64>,
    absorbed: BTreeMap<UtteranceId, u64>,
    ends: BTreeMap<UtteranceId, Vec<u64>>,
    first: Vec<f64>,
    done: Vec<f64>,
    joins: u64,
    second_pass: Vec<SecondPass>,
}

/// How the second pass went for the lines that reached a final.
#[derive(Serialize)]
pub struct SecondPassSummary {
    pub outcomes: BTreeMap<&'static str, u64>,
    /// Lines whose final text differs from the live text.
    pub changed: u64,
    pub refine_ms_p50: Option<f64>,
    pub refine_ms_p90: Option<f64>,
    pub decode_ms_p50: Option<f64>,
    pub decode_ms_p90: Option<f64>,
}

#[derive(Serialize)]
pub struct Summary<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub clip: &'a str,
    pub start: f64,
    pub dur: f64,
    pub clauses: u64,
    pub cuts: BTreeMap<&'static str, u64>,
    pub status: BTreeMap<&'static str, u64>,
    pub lag_first_median: Option<f64>,
    pub lag_done_median: Option<f64>,
    pub lag_done_p90: Option<f64>,
    /// Lines the translation queue joined into an earlier one.
    pub joins: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_pass: Option<SecondPassSummary>,
}

impl ReplayReport {
    pub fn new() -> Self {
        Self {
            status: [
                "final",
                "joined",
                "skipped",
                "passthrough",
                "untranslated",
                "dropped",
                "failed",
            ]
            .map(|key| (key, 0))
            .into(),
            absorbed: BTreeMap::new(),
            ends: BTreeMap::new(),
            first: Vec::new(),
            done: Vec::new(),
            joins: 0,
            second_pass: Vec::new(),
        }
    }
    fn terminal(&mut self, id: UtteranceId, state: &'static str) -> u64 {
        let count = self.absorbed.remove(&id).unwrap_or(0) + 1;
        *self.status.entry(state).or_default() += count;
        count
    }
    pub fn on_event(&mut self, event: &PipelineEvent) {
        if let PipelineEvent::AsrFinal {
            second_pass: Some(pass),
            ..
        } = event
        {
            self.second_pass.push(pass.clone());
        }
        match event {
            PipelineEvent::Joined { id, absorbed, .. } => {
                self.joins += absorbed.len() as u64;
                let mut count = self.absorbed.remove(id).unwrap_or(0);
                let mut ends = self.ends.remove(id).unwrap_or_default();
                for child in absorbed {
                    count += self.absorbed.remove(child).unwrap_or(0) + 1;
                    ends.extend(self.ends.remove(child).unwrap_or_default());
                }
                self.ends.insert(*id, ends);
                self.absorbed.insert(*id, count);
            }
            PipelineEvent::TranslationFinal { id, timing, .. } => {
                let state = if self.absorbed.contains_key(id) {
                    "joined"
                } else {
                    "final"
                };
                let constituents = self.terminal(*id, state);
                // The Python summary measures every constituent from its own
                // end, including members of queue joins. Preserve that policy.
                let ends = self
                    .ends
                    .remove(id)
                    .unwrap_or_else(|| vec![timing.speech_end_ms]);
                for end in ends {
                    self.done
                        .push(timing.done_ms.saturating_sub(end) as f64 / 1000.0);
                    if let Some(first) = timing.first_token_ms {
                        self.first.push(first.saturating_sub(end) as f64 / 1000.0);
                    }
                }
                tracing::debug!(id = id.0, constituents, "replay terminal");
            }
            PipelineEvent::Skipped { id, .. } => {
                self.terminal(*id, "skipped");
                self.ends.remove(id);
            }
            PipelineEvent::TranslationFailed { id, .. } => {
                self.terminal(*id, "failed");
                self.ends.remove(id);
            }
            PipelineEvent::Dropped { id, .. } => {
                self.terminal(*id, "dropped");
                self.ends.remove(id);
            }
            PipelineEvent::AsrFinal {
                id,
                class: TextClass::English,
                ..
            } => {
                self.terminal(*id, "passthrough");
            }
            PipelineEvent::AsrFinal {
                id,
                class: TextClass::Other,
                ..
            } => {
                self.terminal(*id, "untranslated");
            }
            PipelineEvent::AsrFinal { id, end_ms, .. } => {
                self.ends.insert(*id, vec![*end_ms]);
            }
            _ => {}
        }
    }
    pub fn summary<'a>(
        self,
        counts: ClauseCounts,
        clip: &'a str,
        start: f64,
        dur: f64,
    ) -> Summary<'a> {
        Summary {
            kind: "summary",
            clip,
            start,
            dur,
            clauses: counts.total(),
            cuts: [
                ("pause", counts.pause),
                ("discontinuity", counts.discontinuity),
                ("end", counts.end),
                ("commit", counts.commit),
            ]
            .into(),
            status: self.status,
            lag_first_median: percentile(self.first, 0.5),
            lag_done_median: percentile(self.done.clone(), 0.5),
            lag_done_p90: percentile(self.done, 0.9),
            joins: self.joins,
            second_pass: second_pass_summary(&self.second_pass),
        }
    }
}

fn second_pass_summary(passes: &[SecondPass]) -> Option<SecondPassSummary> {
    if passes.is_empty() {
        return None;
    }
    let mut outcomes: BTreeMap<&'static str, u64> = ["used", "mismatch", "late", "failed"]
        .map(|key| (key, 0))
        .into();
    for pass in passes {
        let key = match pass.outcome {
            SecondPassOutcome::Used => "used",
            SecondPassOutcome::Mismatch => "mismatch",
            SecondPassOutcome::Late => "late",
            SecondPassOutcome::Failed => "failed",
        };
        *outcomes.entry(key).or_default() += 1;
    }
    let refine: Vec<f64> = passes
        .iter()
        .map(|pass| f64::from(pass.refine_ms))
        .collect();
    let decode: Vec<f64> = passes
        .iter()
        .filter_map(|pass| pass.decode_ms.map(f64::from))
        .collect();
    Some(SecondPassSummary {
        outcomes,
        changed: passes
            .iter()
            .filter(|pass| pass.live_text.is_some())
            .count() as u64,
        refine_ms_p50: percentile(refine.clone(), 0.5),
        refine_ms_p90: percentile(refine, 0.9),
        decode_ms_p50: percentile(decode.clone(), 0.5),
        decode_ms_p90: percentile(decode, 0.9),
    })
}

fn percentile(mut samples: Vec<f64>, fraction: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f64::total_cmp);
    let at = (samples.len() - 1) as f64 * fraction;
    let left = at.floor() as usize;
    let right = at.ceil() as usize;
    Some(samples[left] + (samples[right] - samples[left]) * at.fract())
}
