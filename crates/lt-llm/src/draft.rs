//! The draft translator (LMT-60 0.6B behind its own llama-server): non-streamed requests with
//! a short budget, and a raw `/completion` path that continues from English already on screen.

use crate::client::{local_agent, map_http_error, translation_error, Endpoint};
use lt_core::{
    config::{Config, LatencyConfig},
    engines::{TranslateRequest, TranslationControl, TranslationOut, Translator, TranslatorCaps},
    error::{Error, Result},
    events::FailReason,
    types::UtteranceId,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// LMT prompt pieces; the user prompt is byte-exact (tested).
#[derive(Clone, Copy, Debug, Default)]
pub struct LmtPrompts;

impl LmtPrompts {
    pub const PREFIX: &'static str =
        "Translate the following text from Chinese into English:\nChinese: ";
    pub const SUFFIX: &'static str = "\nEnglish:";

    pub fn user_message(text: &str) -> String {
        let mut prompt =
            String::with_capacity(Self::PREFIX.len() + text.len() + Self::SUFFIX.len());
        prompt.push_str(Self::PREFIX);
        prompt.push_str(text);
        prompt.push_str(Self::SUFFIX);
        prompt
    }

    /// The chat template written out for `/completion`, ending with the prefill.
    pub fn completion_prompt(text: &str, prefill: &str) -> String {
        format!(
            "<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n{prefill}",
            Self::user_message(text)
        )
    }
}

/// Output budget when the caller gives none.
const DEFAULT_MAX_TOKENS: u32 = 64;

pub struct LmtDraftTranslator {
    endpoint: Endpoint,
    timeout_s: f32,
}

impl LmtDraftTranslator {
    pub fn new(base_url: &str, config: &LatencyConfig) -> Result<Self> {
        Ok(Self {
            endpoint: Endpoint::new(base_url)?,
            timeout_s: config.draft_timeout_s,
        })
    }

    /// Uses `latency.draft_server_url`, which the app fills from the draft supervisor.
    pub fn from_config(config: &Config) -> Result<Self> {
        if config.latency.draft_server_url.is_empty() {
            return Err(Error::Config(
                "Start the draft translation server before building its client".into(),
            ));
        }
        Self::new(&config.latency.draft_server_url, &config.latency)
    }

    fn post(&self, url: &str, body: Value, request: &TranslateRequest<'_>) -> Result<Value> {
        request.control.check()?;
        let control = TranslationControl {
            deadline: request
                .control
                .deadline
                .min(Instant::now() + Duration::from_secs_f32(self.timeout_s)),
            ..request.control.clone()
        };
        let agent = local_agent(self.endpoint.address, control.clone())?;
        let mut response = agent
            .post(url)
            .send_json(body)
            .map_err(|error| map_http_error(error, &control))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::Translation {
                reason: if status >= 500 {
                    FailReason::ServerUnavailable
                } else {
                    FailReason::Error
                },
                message: format!("Draft server returned HTTP {status}"),
            });
        }
        let value: Value = response
            .body_mut()
            .read_json()
            .map_err(|error| map_http_error(error, &control))?;
        control.check()?;
        Ok(value)
    }

    fn translate_inner(&self, request: &TranslateRequest<'_>) -> Result<TranslationOut> {
        if request.tgt != "en" {
            return Err(translation_error(
                "The draft translator supports English output only",
            ));
        }
        if request.text.chars().count() > 300 {
            return Err(translation_error(
                "Translation input exceeds the 300 character limit",
            ));
        }
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        if request.prefill.is_empty() {
            let body = json!({
                "messages": [{"role": "user", "content": LmtPrompts::user_message(request.text)}],
                "stream": false,
                "temperature": 0,
                "max_tokens": max_tokens,
                "cache_prompt": true,
            });
            let value = self.post(&self.endpoint.completions, body, request)?;
            let text = value["choices"][0]["message"]["content"]
                .as_str()
                .ok_or_else(|| translation_error("Draft server returned no text"))?
                .trim()
                .to_owned();
            let usage = &value["usage"];
            Ok(TranslationOut {
                text,
                prompt_tokens: count(&usage["prompt_tokens"]),
                cached_tokens: count(&value["timings"]["cache_n"]),
                generated_tokens: count(&usage["completion_tokens"]),
            })
        } else {
            let body = json!({
                "prompt": LmtPrompts::completion_prompt(request.text, request.prefill),
                "n_predict": max_tokens,
                "temperature": 0,
                "cache_prompt": true,
                "stop": ["<|im_end|>", "\n\n"],
                "stream": false,
            });
            let value = self.post(&self.endpoint.completion_url(), body, request)?;
            // Only the continuation: the caller prepends the prefill.
            let text = value["content"]
                .as_str()
                .ok_or_else(|| translation_error("Draft server returned no text"))?
                .to_owned();
            Ok(TranslationOut {
                text,
                prompt_tokens: count(&value["tokens_evaluated"]),
                cached_tokens: count(&value["tokens_cached"]),
                generated_tokens: count(&value["tokens_predicted"]),
            })
        }
    }
}

fn count(value: &Value) -> u32 {
    value
        .as_u64()
        .map_or(0, |value| u32::try_from(value).unwrap_or(u32::MAX))
}

impl Translator for LmtDraftTranslator {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            streaming: false,
            glossary: false,
            context: false,
            prefill: true,
            max_input_chars: 300,
            pairs: vec![("zh".into(), "en".into())],
        }
    }

    fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
        let request = TranslateRequest {
            id: UtteranceId(0),
            text: "你好。",
            src: "zh",
            tgt: "en",
            terms: &[],
            context: &[],
            prefill: "",
            max_tokens: Some(16),
            control: control.clone(),
        };
        self.translate_inner(&request)?;
        Ok(())
    }

    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        _on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        let span = tracing::debug_span!("draft_http", id = request.id.0);
        let _entered = span.enter();
        self.translate_inner(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_is_byte_identical_to_the_frozen_template() {
        assert_eq!(
            LmtPrompts::user_message("我也想办一个，伟大的公司。"),
            "Translate the following text from Chinese into English:\nChinese: 我也想办一个，伟大的公司。\nEnglish:"
        );
    }

    #[test]
    fn completion_prompt_writes_the_chat_template_and_ends_with_the_prefill() {
        assert_eq!(
            LmtPrompts::completion_prompt("你好。", "Hello, "),
            "<|im_start|>user\nTranslate the following text from Chinese into English:\nChinese: 你好。\nEnglish:<|im_end|>\n<|im_start|>assistant\nHello, "
        );
    }
}
