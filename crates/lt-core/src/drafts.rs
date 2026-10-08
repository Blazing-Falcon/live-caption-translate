//! Draft translations:
//! the draft slot with its growth rule, prefill, publish rules and the worker thread that
//! calls the small translator.

use crate::{
    bus::EventBus,
    config::LatencyConfig,
    draft::{clean_draft, draft_max_tokens, prefill_prefix, reject, word_chars},
    engines::{TranslateRequest, TranslationControl, Translator},
    error::{Error, Result},
    events::{EngineKind, EngineState, PipelineEvent},
    types::UtteranceId,
};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Ids kept after they finish, so a late draft cannot be published for them.
const CLOSED_HISTORY: usize = 512;

/// A partial worth translating, waiting in the single draft slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftJob {
    pub id: UtteranceId,
    pub zh: String,
    pub lang: String,
    pub end_ms: u64,
}

/// A job ready to send: the prefill is computed when the request leaves the slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftRequest {
    pub job: DraftJob,
    pub prefill: String,
}

#[derive(Debug)]
pub enum DraftResult {
    /// Cleaned English, ready to publish.
    Text(String),
    /// The model answered but the text is unusable (empty, copy of the source, untranslated).
    Rejected,
    /// Timeout or server error.
    Failed(String),
}

#[derive(Debug)]
pub struct DraftOutcome {
    pub request: DraftRequest,
    pub result: DraftResult,
}

/// A draft the scheduler should publish.
#[derive(Debug, PartialEq, Eq)]
pub struct PublishedDraft {
    pub id: UtteranceId,
    pub rev: u32,
    pub text: String,
    pub end_ms: u64,
}

#[derive(Default)]
struct ClauseDrafts {
    /// Word characters of the last partial a draft was requested for.
    last_sent_chars: usize,
    rev: u32,
    /// The Chinese and cleaned English of the last published draft, for the prefill.
    last: Option<(String, String)>,
}

pub struct DraftState {
    config: LatencyConfig,
    slot: Option<DraftJob>,
    clauses: BTreeMap<UtteranceId, ClauseDrafts>,
    closed: BTreeSet<UtteranceId>,
    published_total: u32,
    failed_total: u32,
}

impl DraftState {
    pub fn new(config: &LatencyConfig) -> Self {
        Self {
            config: config.clone(),
            slot: None,
            clauses: BTreeMap::new(),
            closed: BTreeSet::new(),
            published_total: 0,
            failed_total: 0,
        }
    }

    pub fn published_total(&self) -> u32 {
        self.published_total
    }

    pub fn failed_total(&self) -> u32 {
        self.failed_total
    }

    pub fn has_job(&self) -> bool {
        self.slot.is_some()
    }

    pub fn clear_slot(&mut self) {
        self.slot = None;
    }

    /// A partial for an open clause. Replaces the waiting job when the growth rule is met.
    pub fn partial(&mut self, id: UtteranceId, text: &str, lang: &str, end_ms: u64) {
        if self.closed.contains(&id) {
            return;
        }
        let chars = word_chars(text);
        let clause = self.clauses.entry(id).or_default();
        let min = self.config.draft_min_chars as usize;
        let grow = self.config.draft_grow_chars as usize;
        if chars >= min && chars >= clause.last_sent_chars + grow {
            self.slot = Some(DraftJob {
                id,
                zh: text.to_owned(),
                lang: lang.to_owned(),
                end_ms,
            });
        }
    }

    /// Take the waiting job and compute its prefill from the clause's previous draft.
    pub fn take_request(&mut self) -> Option<DraftRequest> {
        let job = self.slot.take()?;
        let clause = self.clauses.entry(job.id).or_default();
        clause.last_sent_chars = word_chars(&job.zh);
        let (prev_zh, prev_en) = match &clause.last {
            Some((zh, en)) => (Some(zh.as_str()), Some(en.as_str())),
            None => (None, None),
        };
        let prefill = prefill_prefix(
            prev_zh,
            prev_en,
            &job.zh,
            self.config.draft_keep_back_words as usize,
        );
        Some(DraftRequest { job, prefill })
    }

    /// The clause got its `AsrFinal`: no more partial-driven drafts, drop its waiting job.
    pub fn final_arrived(&mut self, id: UtteranceId) {
        if self.slot.as_ref().is_some_and(|job| job.id == id) {
            self.slot = None;
        }
        // The prefill is only for the open clause.
        if let Some(clause) = self.clauses.get_mut(&id) {
            clause.last = None;
        }
    }

    /// The id reached a terminal event, was dropped or was absorbed by a join.
    pub fn terminal(&mut self, id: UtteranceId) {
        if self.slot.as_ref().is_some_and(|job| job.id == id) {
            self.slot = None;
        }
        self.clauses.remove(&id);
        self.closed.insert(id);
        while self.closed.len() > CLOSED_HISTORY {
            self.closed.pop_first();
        }
    }

    /// A draft request came back. Returns what to publish, if anything.
    pub fn completed(&mut self, outcome: DraftOutcome) -> Option<PublishedDraft> {
        let DraftOutcome { request, result } = outcome;
        let id = request.job.id;
        match result {
            DraftResult::Failed(message) => {
                tracing::debug!(id = id.0, message, "draft failed");
                self.failed_total = self.failed_total.saturating_add(1);
                None
            }
            DraftResult::Rejected => None,
            DraftResult::Text(text) => {
                if self.closed.contains(&id) {
                    return None;
                }
                let clause = self.clauses.entry(id).or_default();
                clause.rev += 1;
                clause.last = Some((request.job.zh.clone(), text.clone()));
                self.published_total = self.published_total.saturating_add(1);
                Some(PublishedDraft {
                    id,
                    rev: clause.rev,
                    text,
                    end_ms: request.job.end_ms,
                })
            }
        }
    }
}

/// Runs the draft translator off the scheduler thread. One request at a time; the scheduler
/// holds back finals' dispatch while a draft is in flight.
pub struct DraftWorker {
    pub input: Sender<DraftRequest>,
    pub completed: Receiver<DraftOutcome>,
    /// `WORKER_LOADING`, `WORKER_READY` or `WORKER_FAILED`: the result of the warm-up.
    pub state: Arc<AtomicU8>,
    join: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
}

pub const WORKER_LOADING: u8 = 0;
pub const WORKER_READY: u8 = 1;
pub const WORKER_FAILED: u8 = 2;

fn translate_draft(
    translator: &mut dyn Translator,
    request: DraftRequest,
    config: &LatencyConfig,
    cancelled: &Arc<AtomicBool>,
) -> DraftOutcome {
    let zh = request.job.zh.clone();
    let control = TranslationControl {
        deadline: Instant::now() + Duration::from_secs_f32(config.draft_timeout_s),
        cancelled: cancelled.clone(),
        abort: Arc::new(AtomicBool::new(false)),
    };
    let wire = TranslateRequest {
        id: request.job.id,
        text: &zh,
        src: &request.job.lang,
        tgt: "en",
        terms: &[],
        context: &[],
        prefill: &request.prefill,
        max_tokens: Some(draft_max_tokens(&zh)),
        control: control.clone(),
    };
    let result = control
        .check()
        .and_then(|()| translator.translate(&wire, &mut |_| {}))
        .and_then(|out| control.check().map(|()| out));
    let result = match result {
        Ok(out) => {
            let cleaned = clean_draft(&format!("{}{}", request.prefill, out.text));
            match reject(&zh, &cleaned) {
                None => DraftResult::Text(cleaned),
                Some(_) => DraftResult::Rejected,
            }
        }
        Err(error) => DraftResult::Failed(error.to_string()),
    };
    DraftOutcome { request, result }
}

impl DraftWorker {
    pub fn start(
        mut translator: Box<dyn Translator>,
        config: LatencyConfig,
        bus: EventBus,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self> {
        let (input, work): (Sender<DraftRequest>, Receiver<DraftRequest>) = bounded(1);
        let (complete, completed) = bounded(1);
        let worker_cancelled = cancelled.clone();
        let state = Arc::new(AtomicU8::new(WORKER_LOADING));
        let worker_state = state.clone();
        let join = thread::Builder::new()
            .name("lt-draft".into())
            .spawn(move || {
                bus.publish(PipelineEvent::EngineStatus {
                    engine: EngineKind::DraftTranslator,
                    state: EngineState::Loading,
                    message: None,
                });
                let control = TranslationControl {
                    deadline: Instant::now() + Duration::from_secs(60),
                    cancelled: worker_cancelled.clone(),
                    abort: Arc::new(AtomicBool::new(false)),
                };
                let warmup = translator.warm_up(&control);
                worker_state.store(
                    if warmup.is_ok() {
                        WORKER_READY
                    } else {
                        WORKER_FAILED
                    },
                    Ordering::Release,
                );
                bus.publish(PipelineEvent::EngineStatus {
                    engine: EngineKind::DraftTranslator,
                    state: if warmup.is_ok() {
                        EngineState::Ready
                    } else {
                        EngineState::Failed
                    },
                    message: warmup.err().map(|error| error.to_string()),
                });
                while !worker_cancelled.load(Ordering::Relaxed) {
                    let request = match work.recv_timeout(Duration::from_millis(20)) {
                        Ok(request) => request,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
                    let outcome =
                        translate_draft(translator.as_mut(), request, &config, &worker_cancelled);
                    let mut pending = outcome;
                    loop {
                        if worker_cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        match complete.send_timeout(pending, Duration::from_millis(20)) {
                            Ok(()) => break,
                            Err(crossbeam_channel::SendTimeoutError::Timeout(back)) => {
                                pending = back;
                            }
                            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => return,
                        }
                    }
                }
            })?;
        Ok(Self {
            input,
            completed,
            state,
            join: Some(join),
            cancelled,
        })
    }

    pub fn stop(&mut self) -> Result<()> {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|_| Error::Engine("Draft worker stopped unexpectedly".into()))?;
        }
        Ok(())
    }
}

impl Drop for DraftWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> DraftState {
        DraftState::new(&LatencyConfig::default())
    }

    fn id(n: u64) -> UtteranceId {
        UtteranceId(n)
    }

    fn done(request: DraftRequest, text: &str) -> DraftOutcome {
        DraftOutcome {
            request,
            result: DraftResult::Text(text.into()),
        }
    }

    #[test]
    fn first_draft_needs_three_word_characters() {
        let mut s = state();
        s.partial(id(1), "你好", "zh", 1_000);
        assert!(!s.has_job());
        s.partial(id(1), "你好吗", "zh", 1_100);
        assert!(s.has_job());
    }

    #[test]
    fn growth_rule_needs_three_more_characters_than_the_last_request() {
        let mut s = state();
        s.partial(id(1), "你好吗我", "zh", 1_000);
        let request = s.take_request().unwrap();
        assert_eq!(request.job.zh, "你好吗我");
        assert_eq!(request.prefill, "");
        s.partial(id(1), "你好吗我很", "zh", 1_500);
        s.partial(id(1), "你好吗我很好", "zh", 1_600);
        assert!(!s.has_job(), "only two new characters");
        s.partial(id(1), "你好吗我很好呀", "zh", 1_700);
        assert!(s.has_job());
    }

    #[test]
    fn only_the_newest_waiting_partial_is_kept() {
        let mut s = state();
        s.partial(id(1), "你好吗", "zh", 1_000);
        s.partial(id(2), "我很好呀", "zh", 1_100);
        let request = s.take_request().unwrap();
        assert_eq!(request.job.id, id(2));
        assert!(s.take_request().is_none());
    }

    #[test]
    fn publishing_counts_revisions_and_feeds_the_next_prefill() {
        let mut s = state();
        s.partial(id(1), "正好我没看。", "zh", 1_000);
        let first = s.take_request().unwrap();
        let published = s.completed(done(first, "I didn't see it.")).unwrap();
        assert_eq!((published.rev, published.end_ms), (1, 1_000));
        s.partial(id(1), "正好我没看过这个电。", "zh", 1_500);
        let second = s.take_request().unwrap();
        assert_eq!(second.prefill, "I didn't ");
        let published = s
            .completed(done(second, "I didn't watch this movie."))
            .unwrap();
        assert_eq!(published.rev, 2);
        assert_eq!(s.published_total(), 2);
    }

    #[test]
    fn nothing_is_published_after_a_terminal_event_or_absorption() {
        let mut s = state();
        s.partial(id(1), "你好吗", "zh", 1_000);
        let request = s.take_request().unwrap();
        s.terminal(id(1));
        assert!(s.completed(done(request, "Hello")).is_none());
        s.partial(id(1), "你好吗我很好", "zh", 2_000);
        assert!(!s.has_job(), "no more jobs for a finished id");
    }

    #[test]
    fn a_draft_landing_after_the_final_is_still_published_once() {
        let mut s = state();
        s.partial(id(1), "你好吗", "zh", 1_000);
        let request = s.take_request().unwrap();
        s.partial(id(1), "你好吗我很", "zh", 1_500);
        s.final_arrived(id(1));
        assert!(!s.has_job(), "the waiting job left with the final");
        assert!(s.completed(done(request, "Hello")).is_some());
    }

    #[test]
    fn failures_are_counted_and_publish_nothing() {
        let mut s = state();
        s.partial(id(1), "你好吗", "zh", 1_000);
        let request = s.take_request().unwrap();
        let outcome = DraftOutcome {
            request,
            result: DraftResult::Failed("timeout".into()),
        };
        assert!(s.completed(outcome).is_none());
        assert_eq!(s.failed_total(), 1);
        s.partial(id(1), "你好吗我很好", "zh", 1_500);
        let request = s.take_request().unwrap();
        let outcome = DraftOutcome {
            request,
            result: DraftResult::Rejected,
        };
        assert!(s.completed(outcome).is_none());
        assert_eq!(s.failed_total(), 1, "a rejected draft is not a failure");
    }

    #[test]
    fn worker_prepends_the_prefill_cleans_and_rejects() {
        struct Scripted(&'static str);
        impl Translator for Scripted {
            fn caps(&self) -> crate::engines::TranslatorCaps {
                crate::engines::TranslatorCaps {
                    streaming: false,
                    glossary: false,
                    context: false,
                    prefill: true,
                    max_input_chars: 300,
                    pairs: vec![],
                }
            }
            fn warm_up(&mut self, _: &TranslationControl) -> Result<()> {
                Ok(())
            }
            fn translate(
                &mut self,
                request: &TranslateRequest<'_>,
                _: &mut dyn FnMut(&str),
            ) -> Result<crate::engines::TranslationOut> {
                assert_eq!(request.max_tokens, Some(draft_max_tokens(request.text)));
                Ok(crate::engines::TranslationOut {
                    text: self.0.into(),
                    ..Default::default()
                })
            }
        }
        let config = LatencyConfig::default();
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = |prefill: &str| DraftRequest {
            job: DraftJob {
                id: id(1),
                zh: "你好吗".into(),
                lang: "zh".into(),
                end_ms: 10,
            },
            prefill: prefill.into(),
        };
        let outcome = translate_draft(
            &mut Scripted("watch it."),
            request("I didn't "),
            &config,
            &cancelled,
        );
        assert!(matches!(outcome.result, DraftResult::Text(t) if t == "I didn't watch it."));
        let outcome = translate_draft(
            &mut Scripted("he he he he he"),
            request(""),
            &config,
            &cancelled,
        );
        assert!(matches!(outcome.result, DraftResult::Text(t) if t == "he"));
        let outcome = translate_draft(&mut Scripted("你好吗"), request(""), &config, &cancelled);
        assert!(matches!(outcome.result, DraftResult::Rejected));
        let outcome = translate_draft(&mut Scripted("  "), request(""), &config, &cancelled);
        assert!(matches!(outcome.result, DraftResult::Rejected));
    }
}
