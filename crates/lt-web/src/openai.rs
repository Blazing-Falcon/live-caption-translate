use crate::http;
use lt_core::{
    config::OpenAiConfig,
    engines::{
        answer_problem, PreviewMode, TranslateRequest, TranslationControl, TranslationOut,
        Translator, TranslatorCaps,
    },
    error::{Error, Result},
    events::FailReason,
    sse::{SseDecoder, SseEvent},
};
use serde_json::{json, Value};
use std::{io::Read, sync::atomic::Ordering, time::Instant};

pub const DEFAULT_PROMPT: &str =
    "Translate the user's Chinese text into natural English. Output only the translation.";

pub struct OpenAiTranslator {
    config: OpenAiConfig,
    url: String,
    agent: ureq::Agent,
    temperature: bool,
}

impl OpenAiTranslator {
    pub fn new(config: &OpenAiConfig) -> Result<Self> {
        let base = config.base_url.trim().trim_end_matches('/');
        let uri: ureq::http::Uri = base
            .parse()
            .map_err(|_| http::error("Enter a valid OpenAI-compatible base URL"))?;
        if !matches!(uri.scheme_str(), Some("http" | "https"))
            || uri.host().is_none()
            || uri
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@'))
            || uri.query().is_some()
            || base.contains('#')
        {
            return Err(http::error(
                "Enter an HTTP or HTTPS base URL without credentials, query or fragment",
            ));
        }
        if config.model.trim().is_empty() {
            return Err(http::error("Enter the OpenAI-compatible model name"));
        }
        let url = if base.ends_with("/chat/completions") {
            base.to_owned()
        } else {
            format!("{base}/chat/completions")
        };
        Ok(Self {
            config: config.clone(),
            url,
            agent: http::agent(),
            temperature: true,
        })
    }

    fn stream(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        request.control.check()?;
        let control = TranslationControl {
            deadline: request
                .control
                .deadline
                .min(Instant::now() + std::time::Duration::from_secs(8)),
            ..request.control.clone()
        };
        let mut body = json!({
            "model": self.config.model,
            "messages": [
                {"role": "system", "content": if self.config.prompt.is_empty() { DEFAULT_PROMPT } else { &self.config.prompt }},
                {"role": "user", "content": request.text}
            ],
            "stream": true
        });
        if self.temperature {
            body["temperature"] = json!(0);
        }
        let mut response = loop {
            control.check()?;
            let mut call = self
                .agent
                .post(&self.url)
                .config()
                .timeout_global(Some(http::timeout(&control)?))
                .build()
                .header("Accept", "text/event-stream");
            if !self.config.api_key.is_empty() {
                let mut bearer = ureq::http::HeaderValue::from_str(&format!(
                    "Bearer {}",
                    self.config.api_key
                ))
                .map_err(|_| http::error("The API key contains invalid header characters"))?;
                bearer.set_sensitive(true);
                call = call.header("Authorization", bearer);
            }
            let mut response = call
                .send_json(&body)
                .map_err(|error| http::map_error(error, &control))?;
            let status = response.status().as_u16();
            if status == 400 && self.temperature {
                let raw = response
                    .body_mut()
                    .read_to_string()
                    .map_err(|error| http::map_error(error, &control))?;
                if raw.to_ascii_lowercase().contains("temperature") {
                    self.temperature = false;
                    body.as_object_mut()
                        .expect("request is an object")
                        .remove("temperature");
                    continue;
                }
            }
            if !(200..300).contains(&status) {
                return Err(Error::Translation {
                    reason: if status == 429 || status >= 500 { FailReason::ServerUnavailable } else { FailReason::Error },
                    message: match status {
                        401 | 403 => "The endpoint rejected the API key. Check the API key and its permissions.".into(),
                        404 => "The endpoint was not found. Check the base URL and model.".into(),
                        _ => format!("Translation endpoint returned HTTP {status}"),
                    }
                });
            }
            break response;
        };
        if response.headers().get("content-type").is_some_and(|value| {
            !value.to_str().is_ok_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("text/event-stream"))
            })
        }) {
            return Err(http::error(
                "The translation endpoint did not return an SSE stream",
            ));
        }
        let mut output = TranslationOut::default();
        let mut decoder = SseDecoder::default();
        let mut reader = response.body_mut().as_reader();
        let mut buffer = [0; 4096];
        loop {
            if graceful_abort(&control, &output) {
                return Ok(output);
            }
            control.check()?;
            let count = reader
                .read(&mut buffer)
                .map_err(|error| http::map_error(error.into(), &control))?;
            if count == 0 {
                return Err(Error::Translation {
                    reason: FailReason::ServerUnavailable,
                    message: "Translation stream ended before [DONE]".into(),
                });
            }
            for byte in &buffer[..count] {
                if let Some(event) = decoder.byte(*byte)? {
                    match event {
                        SseEvent::Done => {
                            control.check()?;
                            return Ok(output);
                        }
                        SseEvent::Data(data) => chunk(&data, &mut output, on_delta)?,
                    }
                    if graceful_abort(&control, &output) {
                        return Ok(output);
                    }
                    control.check()?;
                }
            }
        }
    }
}

impl Translator for OpenAiTranslator {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            previews: PreviewMode::Clauses,
            ..TranslatorCaps::default()
        }
    }
    fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
        control.check()
    }
    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        if request.text.chars().count() > 300 {
            return Err(http::error(
                "Translation input exceeds the 300 character limit",
            ));
        }
        let mut output = self.stream(request, on_delta)?;
        output.text = output.text.trim().to_owned();
        if answer_problem(&output, request.text).is_some() {
            return Err(http::error("Could not translate this line"));
        }
        Ok(output)
    }
}

fn graceful_abort(control: &TranslationControl, output: &TranslationOut) -> bool {
    control.abort.load(Ordering::Relaxed)
        && !control.cancelled.load(Ordering::Relaxed)
        && Instant::now() < control.deadline
        && !output.text.is_empty()
}

fn count(value: &Value) -> u32 {
    value
        .as_u64()
        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

fn chunk(data: &[u8], output: &mut TranslationOut, on_delta: &mut dyn FnMut(&str)) -> Result<()> {
    let chunk: Value = serde_json::from_slice(data)
        .map_err(|_| http::error("Malformed JSON in translation SSE event"))?;
    if chunk.get("error").is_some() {
        return Err(http::error("The translation endpoint rejected the request"));
    }
    if let Some(usage) = chunk.get("usage") {
        output.cached_tokens = count(&usage["prompt_tokens_details"]["cached_tokens"]);
        output.prompt_tokens = count(&usage["prompt_tokens"]).saturating_sub(output.cached_tokens);
        output.generated_tokens = count(&usage["completion_tokens"]);
    }
    if let Some(choices) = chunk["choices"].as_array() {
        for choice in choices {
            if choice["index"].as_u64().is_some_and(|index| index != 0) {
                continue;
            }
            output.truncated |= choice["finish_reason"].as_str() == Some("length");
            let content = &choice["delta"]["content"];
            if content.is_null() {
                continue;
            }
            let text = content
                .as_str()
                .ok_or_else(|| http::error("Invalid SSE delta content"))?;
            if text.is_empty() {
                continue;
            }
            if output.text.len().saturating_add(text.len()) > 64 * 1024 {
                return Err(Error::Translation {
                    reason: FailReason::Runaway,
                    message: "Translation output exceeded its size limit".into(),
                });
            }
            output.text.push_str(text);
            on_delta(&output.text);
        }
    }
    Ok(())
}
