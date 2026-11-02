use crate::{http, WebTranslator};
use lt_core::{
    config::TranslateEngine,
    engines::{TranslateRequest, TranslationControl, TranslationOut, Translator, TranslatorCaps},
    error::{Error, Result},
};

pub(crate) struct Fallback {
    engines: Vec<(&'static str, Box<dyn Translator>)>,
}

impl Fallback {
    pub(crate) fn build(engine: TranslateEngine, primary: Box<dyn Translator>) -> Result<Self> {
        let mut engines: Vec<(&'static str, Box<dyn Translator>)> =
            vec![(engine.as_str(), primary)];
        let backups: &[(&str, TranslateEngine)] = match engine {
            TranslateEngine::Microsoft => &[
                ("google", TranslateEngine::Google),
                ("google_alt", TranslateEngine::Google),
            ],
            TranslateEngine::Google => &[
                ("google_alt", TranslateEngine::Google),
                ("microsoft", TranslateEngine::Microsoft),
            ],
            TranslateEngine::GoogleBeta => &[
                ("google", TranslateEngine::Google),
                ("google_alt", TranslateEngine::Google),
            ],
            TranslateEngine::Bing => &[
                ("google", TranslateEngine::Google),
                ("microsoft", TranslateEngine::Microsoft),
            ],
            TranslateEngine::Youdao => &[("microsoft", TranslateEngine::Microsoft)],
            _ => &[],
        };
        for &(name, engine) in backups {
            engines.push((
                name,
                Box::new(if name == "google_alt" {
                    WebTranslator::google_alt()?
                } else {
                    WebTranslator::new(engine)?
                }),
            ));
        }
        Ok(Self { engines })
    }
}

impl Translator for Fallback {
    fn caps(&self) -> TranslatorCaps {
        self.engines[0].1.caps()
    }

    fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
        control.check()
    }

    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        let answer = self
            .translate_batch(&[request.text], &request.control)?
            .remove(0);
        on_delta(&answer.text);
        Ok(answer)
    }

    fn translate_batch(
        &mut self,
        texts: &[&str],
        control: &TranslationControl,
    ) -> Result<Vec<TranslationOut>> {
        let mut primary_error = None;
        for (position, (name, engine)) in self.engines.iter_mut().enumerate() {
            for _ in 0..if position == 0 { 2 } else { 1 } {
                control.check()?;
                let result = engine.translate_batch(texts, control).and_then(|answers| {
                    if answers.len() == texts.len() {
                        Ok(answers)
                    } else {
                        Err(http::error(
                            "Translation answer count does not match request",
                        ))
                    }
                });
                match result {
                    Ok(answers) => {
                        control.check()?;
                        tracing::info!(engine = name, "Translation answered");
                        return Ok(answers);
                    }
                    Err(Error::Stopped) => return Err(Error::Stopped),
                    Err(error) => {
                        control.check()?;
                        if primary_error.is_none() {
                            primary_error = Some(error);
                        }
                    }
                }
            }
        }
        Err(primary_error.expect("the primary was attempted"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant},
    };

    type Calls = Arc<Mutex<Vec<(String, Vec<String>)>>>;
    struct Fake {
        name: &'static str,
        answers: VecDeque<Result<Vec<TranslationOut>>>,
        calls: Calls,
        cancel: Option<Arc<AtomicBool>>,
    }
    impl Translator for Fake {
        fn caps(&self) -> TranslatorCaps {
            TranslatorCaps {
                max_batch_items: 25,
                ..Default::default()
            }
        }
        fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
            control.check()
        }
        fn translate(
            &mut self,
            _: &TranslateRequest<'_>,
            _: &mut dyn FnMut(&str),
        ) -> Result<TranslationOut> {
            unreachable!()
        }
        fn translate_batch(
            &mut self,
            texts: &[&str],
            _: &TranslationControl,
        ) -> Result<Vec<TranslationOut>> {
            self.calls.lock().unwrap().push((
                self.name.into(),
                texts.iter().map(|s| (*s).into()).collect(),
            ));
            if let Some(cancel) = &self.cancel {
                cancel.store(true, Ordering::Relaxed);
            }
            self.answers.pop_front().unwrap()
        }
    }
    fn control() -> TranslationControl {
        TranslationControl {
            deadline: Instant::now() + Duration::from_secs(5),
            cancelled: Arc::new(AtomicBool::new(false)),
            abort: Arc::new(AtomicBool::new(false)),
        }
    }
    fn good() -> Result<Vec<TranslationOut>> {
        Ok(vec![
            TranslationOut {
                text: "one".into(),
                ..Default::default()
            },
            TranslationOut {
                text: "two".into(),
                ..Default::default()
            },
        ])
    }
    fn bad(message: &str) -> Result<Vec<TranslationOut>> {
        Err(http::error(message))
    }
    fn fake(
        name: &'static str,
        answers: Vec<Result<Vec<TranslationOut>>>,
        calls: &Calls,
    ) -> (&'static str, Box<dyn Translator>) {
        (
            name,
            Box::new(Fake {
                name,
                answers: answers.into(),
                calls: calls.clone(),
                cancel: None,
            }),
        )
    }
    #[test]
    fn primary_retries_then_answers_without_backup() {
        let calls = Calls::default();
        let mut chain = Fallback {
            engines: vec![
                fake("primary", vec![bad("first"), good()], &calls),
                fake("backup", vec![], &calls),
            ],
        };
        let answers = chain.translate_batch(&["一", "二"], &control()).unwrap();
        assert_eq!(answers[1].text, "two");
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .map(|c| c.0.as_str())
                .collect::<Vec<_>>(),
            ["primary", "primary"]
        );
        assert_eq!(chain.caps().max_batch_items, 25);
    }
    #[test]
    fn backup_answers_entire_batch_after_short_primary_reply() {
        let calls = Calls::default();
        let mut chain = Fallback {
            engines: vec![
                fake("primary", vec![Ok(vec![]), bad("retry")], &calls),
                fake("backup", vec![good()], &calls),
            ],
        };
        assert_eq!(
            chain
                .translate_batch(&["一", "二"], &control())
                .unwrap()
                .len(),
            2
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[2], ("backup".into(), vec!["一".into(), "二".into()]));
    }
    #[test]
    fn all_failures_return_the_first_primary_error() {
        let calls = Calls::default();
        let mut chain = Fallback {
            engines: vec![
                fake("primary", vec![bad("original"), bad("retry")], &calls),
                fake("backup", vec![bad("backup")], &calls),
            ],
        };
        assert!(chain
            .translate_batch(&["一", "二"], &control())
            .unwrap_err()
            .to_string()
            .contains("original"));
    }
    #[test]
    fn cancellation_and_stopped_never_reach_another_attempt() {
        let calls = Calls::default();
        let request_control = control();
        let primary = Fake {
            name: "primary",
            answers: vec![bad("network")].into(),
            calls: calls.clone(),
            cancel: Some(request_control.cancelled.clone()),
        };
        let mut chain = Fallback {
            engines: vec![
                ("primary", Box::new(primary)),
                fake("backup", vec![], &calls),
            ],
        };
        assert!(matches!(
            chain.translate_batch(&["一", "二"], &request_control),
            Err(Error::Stopped)
        ));
        assert_eq!(calls.lock().unwrap().len(), 1);
        let mut chain = Fallback {
            engines: vec![
                fake("primary", vec![Err(Error::Stopped)], &calls),
                fake("backup", vec![], &calls),
            ],
        };
        assert!(matches!(
            chain.translate_batch(&["一", "二"], &control()),
            Err(Error::Stopped)
        ));
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
}
