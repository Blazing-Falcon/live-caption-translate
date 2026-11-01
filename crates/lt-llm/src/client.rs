//! Hy-MT2 over the local server's chat completions API.
use crate::{
    http::{
        local_agent, map_http_error, server_unavailable, translation_error, Endpoint, SseDecoder,
        SseEvent,
    },
    prompts::HyMt2Prompts,
};
use lt_core::{
    engines::{
        max_tokens, TranslateRequest, TranslationControl, TranslationOut, Translator,
        TranslatorCaps,
    },
    error::{Error, Result},
    events::FailReason,
    types::UtteranceId,
};
use serde_json::{json, Value};
use std::{io::Read, sync::atomic::Ordering, time::Instant};

const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Hy-MT2's template ends a sentence with a control token. A leaked spelling of it is ordinary
/// text, so a stop string catches it; the model has been seen to emit either full-width bar.
pub const LEAKED_MARKERS: [&str; 6] = [
    "<｜hy_end▁of▁sentence｜>",
    "<｠hy_end▁of▁sentence｠>",
    "<｜hy_place▁holder▁no▁2｜>",
    "<｠hy_place▁holder▁no▁2｠>",
    "<｜end▁of▁message｜>",
    "<｠end▁of▁message｠>",
];

/// Greedy decoding: the translator repeats the prompt at any sampling.
const TEMPERATURE: f32 = 0.0;
const REPEAT_PENALTY: f32 = 1.05;

pub struct OpenAiCompatTranslator {
    endpoint: Endpoint,
}

impl OpenAiCompatTranslator {
    /// `base_url` is the URL the bundled supervisor selected.
    pub fn new(base_url: &str) -> Result<Self> {
        Ok(Self {
            endpoint: Endpoint::new(base_url)?,
        })
    }

    fn translate_inner(
        &self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        if request.text.chars().count() > 300 {
            return Err(translation_error(
                "Translation input exceeds the 300 character limit",
            ));
        }
        let mut output = self.stream(alone(request.text), request, on_delta)?;
        finish(&mut output);
        Ok(output)
    }

    /// One streamed request.
    fn stream(
        &self,
        messages: Vec<Value>,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        request.control.check()?;
        let control = TranslationControl {
            deadline: request
                .control
                .deadline
                .min(Instant::now() + lt_core::engines::REQUEST_TIMEOUT),
            ..request.control.clone()
        };
        let body = json!({
            "messages": messages,
            "stream":true,
            "temperature":readable_float(TEMPERATURE),
            "top_k":1,
            "top_p":1.0,
            "min_p":0.0,
            "repeat_penalty":readable_float(REPEAT_PENALTY),
            "max_tokens":max_tokens(request.text),
            "cache_prompt":true,
            "stream_options":{"include_usage":true},
            "stop":LEAKED_MARKERS,
        });
        let agent = local_agent(self.endpoint.address, control.clone())?;
        let mut response = agent
            .post(&self.endpoint.completions)
            .header("Accept", "text/event-stream")
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
                message: format!("Local translation server returned HTTP {status}"),
            });
        }
        if let Some(content_type) = response.headers().get("content-type") {
            let content_type = content_type
                .to_str()
                .map_err(|_| translation_error("Invalid translation response Content-Type"))?;
            if !content_type
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("text/event-stream"))
            {
                return Err(translation_error(
                    "Local translation server did not return an SSE stream",
                ));
            }
        }
        let mut reader = response.body_mut().as_reader();
        let mut decoder = SseDecoder::default();
        let mut output = TranslationOut::default();
        let mut timings_received = false;
        let mut buffer = [0_u8; 4096];
        loop {
            if graceful_abort(&control, &output) {
                return Ok(output);
            }
            control.check()?;
            let count = match reader.read(&mut buffer) {
                Ok(count) => count,
                Err(error) => {
                    if graceful_abort(&control, &output) {
                        return Ok(output);
                    }
                    return Err(map_http_error(error.into(), &control));
                }
            };
            if count == 0 {
                return Err(server_unavailable("Translation stream ended before [DONE]"));
            }
            for byte in &buffer[..count] {
                if let Some(event) = decoder.byte(*byte)? {
                    match event {
                        SseEvent::Done => {
                            control.check()?;
                            return Ok(output);
                        }
                        SseEvent::Data(data) => {
                            process_chunk(&data, &mut output, &mut timings_received, on_delta)?
                        }
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

impl Translator for OpenAiCompatTranslator {
    fn caps(&self) -> TranslatorCaps {
        TranslatorCaps {
            max_input_chars: 300,
            ..TranslatorCaps::default()
        }
    }

    fn warm_up(&mut self, control: &TranslationControl) -> Result<()> {
        let request = TranslateRequest {
            id: UtteranceId(0),
            text: "你好。",
            max_tokens: None,
            control: control.clone(),
        };
        self.translate_inner(&request, &mut |_| {})?;
        Ok(())
    }

    fn translate(
        &mut self,
        request: &TranslateRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<TranslationOut> {
        let span = tracing::debug_span!("translation_http", id = request.id.0);
        let _entered = span.enter();
        let output = self.translate_inner(request, on_delta)?;
        if let Some(problem) = answer_problem(&output, request) {
            tracing::debug!(id = request.id.0, problem, "Unusable translation answer");
            return Err(translation_error("Could not translate this line"));
        }
        // An answer cut at the output budget is still readable English; the line is shown short
        // rather than replaced by a failure.
        Ok(output)
    }
}

/// One line, as the only user turn.
fn alone(text: &str) -> Vec<Value> {
    vec![json!({"role": "user", "content": HyMt2Prompts::user_message(text)})]
}

/// Trim the answer and drop any leaked template marker, which is ordinary text to us.
fn finish(output: &mut TranslationOut) {
    output.text = strip_marker_residue(output.text.trim());
    output.text = output.text.trim().to_owned();
}

/// Why the answer is not usable as this line's translation, if it is not.
/// The answer is unusable: it is empty, far longer than any translation, or it answers the
/// instruction instead of translating. A leftover Chinese proper noun is not one of these: the
/// pipeline shows it and counts it, so rejecting it would lose a real translation.
fn answer_problem(output: &TranslationOut, request: &TranslateRequest<'_>) -> Option<&'static str> {
    lt_core::engines::answer_problem(output, request.text)
}

/// Remove a leaked marker spelling, including a truncated one at the end of the answer, and a
/// trailing ellipsis.
///
/// A caption ends where a clause ends, and the speaker usually goes on in the next one. Subtitle
/// style does not mark a sentence that continues in the next subtitle with an ellipsis, but the
/// model adds one to a clause that ends at a comma.
fn strip_marker_residue(text: &str) -> String {
    let mut cleaned = text.to_owned();
    for marker in LEAKED_MARKERS {
        if let Some(at) = cleaned.find(marker) {
            cleaned.truncate(at);
        }
    }
    // A marker cut off by the token budget leaves an opening bracket and part of the name.
    for needle in ["<｜hy_", "<｠hy_"] {
        if let Some(at) = cleaned.find(needle) {
            cleaned.truncate(at);
        }
    }
    let mut cleaned = cleaned.trim_end().to_owned();
    for ellipsis in ["…", "..."] {
        if let Some(rest) = cleaned.strip_suffix(ellipsis) {
            cleaned = rest.trim_end().to_owned();
        }
    }
    cleaned
}

fn graceful_abort(control: &TranslationControl, output: &TranslationOut) -> bool {
    control.abort.load(Ordering::Relaxed)
        && !control.cancelled.load(Ordering::Relaxed)
        && Instant::now() < control.deadline
        && !output.text.is_empty()
}

fn count(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
}

fn readable_float(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(f64::from(value))
}

fn process_chunk(
    data: &[u8],
    output: &mut TranslationOut,
    timings_received: &mut bool,
    on_delta: &mut dyn FnMut(&str),
) -> Result<()> {
    let chunk: Value = serde_json::from_slice(data)
        .map_err(|_| translation_error("Malformed JSON in translation SSE event"))?;
    if chunk.get("error").is_some() {
        return Err(translation_error(
            "The local translation server rejected the request",
        ));
    }
    if let Some(usage) = chunk.get("usage").filter(|_| !*timings_received) {
        let cached = count(
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens")),
        )
        .unwrap_or(0);
        if let Some(prompt) = count(usage.get("prompt_tokens")) {
            output.prompt_tokens = prompt.saturating_sub(cached);
        }
        output.cached_tokens = cached;
        if let Some(generated) = count(usage.get("completion_tokens")) {
            output.generated_tokens = generated;
        }
    }
    if let Some(timings) = chunk.get("timings") {
        if ["prompt_n", "cache_n", "predicted_n"]
            .iter()
            .any(|key| count(timings.get(*key)).is_some())
        {
            *timings_received = true;
        }
        if let Some(prompt) = count(timings.get("prompt_n")) {
            output.prompt_tokens = prompt;
        }
        if let Some(cached) = count(timings.get("cache_n")) {
            output.cached_tokens = cached;
        }
        if let Some(generated) = count(timings.get("predicted_n")) {
            output.generated_tokens = generated;
        }
    }
    if let Some(choices) = chunk.get("choices").and_then(Value::as_array) {
        for choice in choices {
            if choice
                .get("index")
                .and_then(Value::as_u64)
                .is_some_and(|index| index != 0)
            {
                continue;
            }
            if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
                output.truncated = true;
            }
            if let Some(content) = choice.get("delta").and_then(|delta| delta.get("content")) {
                if content.is_null() {
                    continue;
                }
                let content = content
                    .as_str()
                    .ok_or_else(|| translation_error("Invalid SSE delta content"))?;
                if content.is_empty() {
                    continue;
                }
                if output.text.len().saturating_add(content.len()) > MAX_OUTPUT_BYTES {
                    return Err(Error::Translation {
                        reason: FailReason::Runaway,
                        message: "Translation output exceeded its size limit".into(),
                    });
                }
                output.text.push_str(content);
                on_delta(&output.text);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod residue_tests {
    use super::{answer_problem, strip_marker_residue};
    use lt_core::{
        engines::{TranslateRequest, TranslationControl, TranslationOut},
        types::UtteranceId,
    };
    use std::{
        sync::{atomic::AtomicBool, Arc},
        time::{Duration, Instant},
    };

    /// A reply to the instruction introduces something with a colon. A translation that only
    /// uses the same words is a translation.
    #[test]
    fn a_reply_is_told_apart_from_a_translation_that_shares_its_words() {
        let request = TranslateRequest {
            id: UtteranceId(1),
            text: "这个翻译是错的。",
            max_tokens: None,
            control: TranslationControl {
                deadline: Instant::now() + Duration::from_secs(1),
                cancelled: Arc::new(AtomicBool::new(false)),
                abort: Arc::new(AtomicBool::new(false)),
            },
        };
        let answer = |text: &str| TranslationOut {
            text: text.into(),
            ..TranslationOut::default()
        };
        assert_eq!(
            answer_problem(&answer("The translation is wrong."), &request),
            None
        );
        assert_eq!(
            answer_problem(&answer("Please translate this for me."), &request),
            None
        );
        assert_eq!(
            answer_problem(
                &answer("Here is the translation: This translation is wrong."),
                &request
            ),
            Some("answered instead of translating")
        );
    }

    /// A caption whose sentence goes on in the next one ends without an ellipsis, whether the
    /// model added it or copied the source's own unfinished mark.
    #[test]
    fn a_trailing_ellipsis_is_removed() {
        assert_eq!(
            strip_marker_residue("Then just assume it's that…"),
            "Then just assume it's that"
        );
        assert_eq!(
            strip_marker_residue("And then, in one place..."),
            "And then, in one place"
        );
        assert_eq!(
            strip_marker_residue("Then that…<｜hy_end▁of▁sentence｜>"),
            "Then that"
        );
        // An ellipsis inside the sentence is the model's own punctuation and stays.
        assert_eq!(strip_marker_residue("I… don't know."), "I… don't know.");
    }

    #[test]
    fn a_leaked_marker_is_cut() {
        assert_eq!(
            strip_marker_residue("Hello.<｜hy_end▁of▁sentence｜>"),
            "Hello."
        );
    }
}
