mod google;
mod google_alt;
mod http;
mod microsoft;
mod openai;
pub use openai::OpenAiTranslator;

use lt_core::{
    config::{TranslateConfig, TranslateEngine},
    engines::{
        answer_problem, PreviewMode, TranslateRequest, TranslationControl, TranslationOut,
        Translator, TranslatorCaps,
    },
    error::Result,
};

pub fn build(config: &TranslateConfig) -> Result<Box<dyn Translator>> {
    if config.engine == TranslateEngine::Openai {
        return Ok(Box::new(OpenAiTranslator::new(&config.openai)?));
    }
    Ok(Box::new(WebTranslator::new(config.engine)?))
}

pub struct WebTranslator {
    engine: TranslateEngine,
    agent: ureq::Agent,
    google_alt: bool,
}

impl WebTranslator {
    pub fn new(engine: TranslateEngine) -> Result<Self> {
        if engine.is_local() || engine == TranslateEngine::Openai {
            return Err(http::error("Local translation requires llama-server"));
        }
        Ok(Self {
            engine,
            agent: http::agent(),
            google_alt: false,
        })
    }

    pub fn google_alt() -> Result<Self> {
        let mut translator = Self::new(TranslateEngine::Google)?;
        translator.google_alt = true;
        Ok(translator)
    }

    pub fn record_response(
        &self,
        texts: &[&str],
        control: &TranslationControl,
    ) -> Result<(String, serde_json::Value)> {
        let caps = self.caps();
        if texts.len() > caps.max_batch_items
            || texts
                .iter()
                .map(|s| s.encode_utf16().count())
                .sum::<usize>()
                > caps.max_batch_chars
        {
            return Err(http::error("Recording exceeds one request's limits"));
        }
        if self.google_alt {
            let (raw, answers) = google_alt::send(&self.agent, texts, control)?;
            return Ok((raw, serde_json::json!({"answers": answers})));
        }
        match self.engine {
            TranslateEngine::Microsoft => microsoft::send(&self.agent, texts, control),
            TranslateEngine::Google => {
                google::send(&self.agent, texts, control).map(|raw| (raw, serde_json::Value::Null))
            }
            TranslateEngine::Local | TranslateEngine::Openai => {
                Err(http::error("Local engine is not an online endpoint"))
            }
        }
    }
}

pub fn parse_google_alt(raw: &str, count: usize) -> Result<Vec<String>> {
    google_alt::parse(raw, count)?
        .into_iter()
        .map(|answer| answer.ok_or_else(|| http::error("Missing Google alternate answer")))
        .collect()
}

pub fn parse_response(engine: TranslateEngine, raw: &str) -> Result<Vec<String>> {
    match engine {
        TranslateEngine::Microsoft => microsoft::parse(raw),
        TranslateEngine::Google => google::parse(raw),
        TranslateEngine::Local | TranslateEngine::Openai => {
            Err(http::error("Local engine is not an online endpoint"))
        }
    }
}

impl Translator for WebTranslator {
    fn caps(&self) -> TranslatorCaps {
        let (items, chars) = match self.engine {
            TranslateEngine::Google => (50, 5000),
            _ => (25, 1000),
        };
        TranslatorCaps {
            max_input_chars: 300,
            previews: PreviewMode::Clauses,
            max_batch_items: items,
            max_batch_chars: chars,
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
        let output = self
            .translate_batch(&[request.text], &request.control)?
            .remove(0);
        on_delta(&output.text);
        Ok(output)
    }

    fn translate_batch(
        &mut self,
        texts: &[&str],
        control: &TranslationControl,
    ) -> Result<Vec<TranslationOut>> {
        control.check()?;
        let caps = self.caps();
        let pieces: Vec<_> = texts
            .iter()
            .enumerate()
            .flat_map(|(owner, text)| {
                split(text, 800.min(caps.max_batch_chars))
                    .into_iter()
                    .map(move |(text, separator)| (owner, text, separator))
            })
            .collect();
        let mut output: Vec<_> = texts
            .iter()
            .map(|text| TranslationOut {
                text: if text.trim().is_empty() {
                    (*text).into()
                } else {
                    String::new()
                },
                ..Default::default()
            })
            .collect();
        let mut start = 0;
        while start < pieces.len() {
            let mut end = start;
            let mut characters = 0;
            while end < pieces.len() && end - start < caps.max_batch_items {
                let next = pieces[end].1.encode_utf16().count();
                if end > start && characters + next > caps.max_batch_chars {
                    break;
                }
                characters += next;
                end += 1;
            }
            let inputs: Vec<_> = pieces[start..end].iter().map(|piece| piece.1).collect();
            let began = std::time::Instant::now();
            let (raw, metadata) = self.record_response(&inputs, control)?;
            let answers = if self.google_alt {
                serde_json::from_value(metadata["answers"].clone())
                    .map_err(|_| http::error("Incomplete Google alternate answers"))?
            } else {
                parse_response(self.engine, &raw)?
            };
            if answers.len() != inputs.len() {
                return Err(http::error(
                    "Translation answer count does not match request",
                ));
            }
            for ((owner, source, separator), answer) in pieces[start..end].iter().zip(answers) {
                let checked = TranslationOut {
                    text: answer.trim().into(),
                    ..Default::default()
                };
                if answer_problem(&checked, source).is_some() {
                    return Err(http::error("Could not translate this line"));
                }
                output[*owner].text.push_str(&checked.text);
                output[*owner].text.push_str(separator);
            }
            tracing::debug!(engine = %self.engine, items = inputs.len(), characters, ms = began.elapsed().as_millis(), "Online translation request");
            start = end;
        }
        control.check()?;
        Ok(output)
    }
}

fn split(text: &str, limit: usize) -> Vec<(&str, &'static str)> {
    let mut rest = text.trim();
    let mut pieces = Vec::new();
    while !rest.is_empty() {
        if rest.encode_utf16().count() <= limit {
            pieces.push((rest, ""));
            break;
        }
        let mut units = 0;
        let mut hard = 0;
        let mut sentence = None;
        let mut whitespace = None;
        for (at, c) in rest.char_indices() {
            units += c.len_utf16();
            if units > limit {
                break;
            }
            hard = at + c.len_utf8();
            if matches!(c, '。' | '！' | '？' | '；')
                || (matches!(c, '.' | '!' | '?')
                    && rest[hard..].chars().next().is_none_or(char::is_whitespace))
            {
                sentence = Some(hard);
            }
            if c.is_whitespace() {
                whitespace = Some(hard);
            }
        }
        let cut = sentence.or(whitespace).unwrap_or(hard);
        let after = &rest[cut..];
        let trimmed = after.trim_start();
        let separator = if after[..after.len() - trimmed.len()].contains("\n\n") {
            "\n\n"
        } else if sentence.is_some() || whitespace.is_some() {
            " "
        } else {
            ""
        };
        pieces.push((rest[..cut].trim(), separator));
        rest = trimmed;
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepted_request_signature_is_reproducible() {
        let recorded: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/microsoft/signature.json"))
                .unwrap();
        assert_eq!(
            microsoft::signature(
                recorded["url"].as_str().unwrap(),
                recorded["date"].as_str().unwrap(),
                recorded["nonce"].as_str().unwrap()
            ),
            recorded["signature"].as_str().unwrap()
        );
    }

    #[test]
    fn splitting_preserves_unicode_and_request_limits() {
        let source = "测😀试".repeat(500);
        let parts = split(&source, 800);
        assert!(parts
            .iter()
            .all(|(text, _)| text.encode_utf16().count() <= 800));
        assert_eq!(
            parts
                .iter()
                .map(|(text, separator)| format!("{text}{separator}"))
                .collect::<String>(),
            source
        );
    }
}
