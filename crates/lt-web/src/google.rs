use crate::http;
use lt_core::{engines::TranslationControl, error::Result};
use serde_json::Value;

pub fn send(agent: &ureq::Agent, texts: &[&str], control: &TranslationControl) -> Result<String> {
    let body = texts
        .iter()
        .map(|text| format!("q={}", http::encode(text)))
        .collect::<Vec<_>>()
        .join("&");
    let response = agent
        .post("https://translate.googleapis.com/translate_a/t?client=gtx&sl=auto&tl=en&format=text")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .config()
        .timeout_global(Some(http::timeout(control)?))
        .build()
        .send(body)
        .map_err(|failure| http::map_error(failure, control))?;
    http::read(response, control)
}

pub fn parse(raw: &str) -> Result<Vec<String>> {
    let value: Value =
        serde_json::from_str(raw).map_err(|_| http::error("Malformed Google response"))?;
    value
        .as_array()
        .ok_or_else(|| http::error("Expected Google answer list"))?
        .iter()
        .map(|item| {
            item.as_str()
                .or_else(|| item[0].as_str())
                .map(str::to_owned)
                .ok_or_else(|| http::error("Missing Google translation"))
        })
        .collect()
}
