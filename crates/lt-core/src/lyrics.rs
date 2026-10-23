//! Marking song lyrics in a stream.
//!
//! A lyric line is one the recognizer is least sure about: singing is outside what the acoustic
//! model was trained on, so its token log-probabilities drop. The cut is relative to the session,
//! because the absolute level moves with the channel and the recording; a fixed number tuned on
//! one clip fires on 7% to 43% of lines under a 0.3 dB shift (`threshold-portability.md`).

use std::collections::VecDeque;

/// Lines kept for the session's own distribution. Enough to be stable, short enough to follow a
/// change of scene.
const WINDOW: usize = 100;
/// How far below the session's median a line must be, in median absolute deviations.
const DEVIATIONS: f32 = 2.5;
/// A single low line is a cough or a misheard word; a run of two is a different kind of audio.
const RUN: usize = 2;
/// No flag before the session has this many lines to compare against.
const COLD_START: usize = 20;

/// Marks lines that look like singing, from the recognizer's confidence in them.
#[derive(Default)]
pub struct LyricDetector {
    /// Log-probabilities of recent lines that were not flagged, newest last.
    recent: VecDeque<f32>,
    /// How many of the most recent lines were below the cut, consecutive.
    run: usize,
}

impl LyricDetector {
    /// Whether this line looks like singing. `logprob` is the line's mean token log-probability,
    /// or `None` when the engine gives none, in which case nothing is marked.
    pub fn observe(&mut self, logprob: Option<f32>) -> bool {
        let Some(logprob) = logprob.filter(|value| value.is_finite()) else {
            self.run = 0;
            return false;
        };
        let cut = self.cut();
        let below = cut.is_some_and(|cut| logprob < cut);
        self.run = if below { self.run + 1 } else { 0 };
        let flagged = self.run >= RUN;
        // A flagged line would pull the session's own median down and hide the next song, so
        // only unflagged lines teach the detector what this recording sounds like.
        if !flagged {
            if self.recent.len() == WINDOW {
                self.recent.pop_front();
            }
            self.recent.push_back(logprob);
        }
        flagged
    }

    /// The line below which a line counts as unusually uncertain, once there is enough of the
    /// session to say what usual is.
    fn cut(&self) -> Option<f32> {
        if self.recent.len() < COLD_START {
            return None;
        }
        let middle = median(self.recent.iter().copied());
        let deviation = median(self.recent.iter().map(|value| (value - middle).abs()));
        Some(middle - DEVIATIONS * deviation)
    }
}

fn median(values: impl Iterator<Item = f32>) -> f32 {
    let mut values: Vec<f32> = values.collect();
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Twenty steady lines, then a drop. The drop is measured against the steady ones.
    #[test]
    fn a_run_of_uncertain_lines_is_marked() {
        let mut detector = LyricDetector::default();
        for _ in 0..COLD_START {
            assert!(!detector.observe(Some(-0.3)));
        }
        assert!(!detector.observe(Some(-1.6)), "one low line is not a song");
        assert!(detector.observe(Some(-1.7)), "the second low line is");
        assert!(detector.observe(Some(-1.5)));
        assert!(!detector.observe(Some(-0.3)), "speech resumes");
    }

    #[test]
    fn nothing_is_marked_before_the_session_has_a_distribution() {
        let mut detector = LyricDetector::default();
        for _ in 0..COLD_START - 1 {
            assert!(!detector.observe(Some(-2.0)));
        }
    }

    #[test]
    fn a_quiet_recording_is_not_all_marked() {
        // Every line is uncertain here; none of them stands out from the others.
        let mut detector = LyricDetector::default();
        let mut flagged = 0;
        for index in 0..60 {
            let jitter = if index % 2 == 0 { -0.02 } else { 0.02 };
            if detector.observe(Some(-1.4 + jitter)) {
                flagged += 1;
            }
        }
        assert_eq!(flagged, 0);
    }

    #[test]
    fn a_missing_logprob_breaks_the_run() {
        let mut detector = LyricDetector::default();
        for _ in 0..COLD_START {
            detector.observe(Some(-0.3));
        }
        assert!(!detector.observe(Some(-1.6)));
        assert!(!detector.observe(None));
        assert!(!detector.observe(Some(-1.6)), "the run started over");
    }
}
