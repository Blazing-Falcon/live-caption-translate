use crate::http;
use lt_core::{
    engines::TranslationControl,
    error::{Error, Result},
    events::FailReason,
};
use regex::Regex;
use serde_json::{json, Value};

#[derive(Default)]
pub struct Session {
    key: Option<String>,
}

impl Session {
    pub fn send(
        &mut self,
        agent: &ureq::Agent,
        texts: &[&str],
        control: &TranslationControl,
    ) -> Result<String> {
        let payload = json!([
            [
                texts.iter().map(|text| escape(text)).collect::<Vec<_>>(),
                "auto",
                "en"
            ],
            "te_lib"
        ]);
        for attempt in 0..2 {
            if self.key.is_none() {
                self.key = Some(lookup(agent, control)?);
            }
            let (status, raw) = post(
                agent,
                &payload,
                self.key.as_deref().expect("looked up"),
                control,
            )?;
            let refused = matches!(status, 401 | 403) || status == 400 && raw.contains("API_KEY_");
            if refused {
                self.key = None;
                if attempt == 0 {
                    continue;
                }
            }
            if !(200..300).contains(&status) {
                return Err(Error::Translation {
                    reason: if status == 429 || status >= 500 {
                        FailReason::ServerUnavailable
                    } else {
                        FailReason::Error
                    },
                    message: format!("Google Beta returned HTTP {status}"),
                });
            }
            return Ok(raw);
        }
        unreachable!()
    }
}

fn post(
    agent: &ureq::Agent,
    payload: &Value,
    key: &str,
    control: &TranslationControl,
) -> Result<(u16, String)> {
    for (attempt, host) in [
        "translate-pa.googleapis.com",
        "translate-pa.us.rep.googleapis.com",
    ]
    .iter()
    .enumerate()
    {
        let mut header = ureq::http::HeaderValue::from_str(key)
            .map_err(|_| http::error("Invalid Google Beta session key"))?;
        header.set_sensitive(true);
        let response = agent
            .post(format!("https://{host}/v1/translateHtml"))
            .config()
            .timeout_global(Some(http::timeout(control)?))
            .build()
            .header("X-Goog-API-Key", header)
            .header("Content-Type", "application/json+protobuf")
            .send(payload.to_string().as_bytes());
        let mut response = match response {
            Ok(response) => response,
            Err(error) if attempt == 0 && control.check().is_ok() => {
                let _ = error;
                continue;
            }
            Err(error) => return Err(http::map_error(error, control)),
        };
        let status = response.status().as_u16();
        if status >= 500 && attempt == 0 {
            continue;
        }
        let raw = response
            .body_mut()
            .read_to_string()
            .map_err(|error| http::map_error(error, control))?;
        control.check()?;
        return Ok((status, raw));
    }
    unreachable!()
}

fn lookup(agent: &ureq::Agent, control: &TranslationControl) -> Result<String> {
    let loader = http::get(
        agent,
        "https://translate.googleapis.com/translate_a/element.js?cb=_",
        None,
        control,
    )?;
    let address = main_script(&loader)?;
    let main = http::get(agent, &address, None, control)?;
    keys(&main)?
        .into_iter()
        .next()
        .ok_or_else(|| http::error("No Google Beta key in element script"))
}

fn main_script(loader: &str) -> Result<String> {
    let loader = loader
        .replace("\\/", "/")
        .replace("\\\\u003d", "=")
        .replace("\\u003d", "=");
    Regex::new(r#"https://translate\.googleapis\.com/_/translate_http/_/js/[^"'\\\s]+"#)
        .expect("fixed regex")
        .find(&loader)
        .map(|found| found.as_str().to_owned())
        .ok_or_else(|| http::error("No main script in Google Beta loader"))
}

fn keys(main: &str) -> Result<Vec<String>> {
    let pattern = Regex::new(r"AIza[0-9A-Za-z_\-]{35}").expect("fixed regex");
    let path = main.find("translateHtml");
    let mut keys: Vec<_> = pattern.find_iter(main).collect();
    keys.sort_by_key(|found| {
        path.filter(|path| found.start() > *path)
            .map_or(usize::MAX, |path| found.start() - path)
    });
    Ok(keys
        .into_iter()
        .map(|found| found.as_str().to_owned())
        .collect())
}

fn escape(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn unescape(text: &str) -> String {
    let numeric = Regex::new(r"&#(x[0-9A-Fa-f]+|[0-9]+);").expect("fixed regex");
    numeric
        .replace_all(text, |capture: &regex::Captures<'_>| {
            let digits = &capture[1];
            let number = if let Some(hex) = digits.strip_prefix('x') {
                u32::from_str_radix(hex, 16).ok()
            } else {
                digits.parse().ok()
            };
            number
                .and_then(char::from_u32)
                .map_or_else(|| capture[0].to_owned(), |c| c.to_string())
        })
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", "\u{a0}")
        .replace("&amp;", "&")
}

pub fn parse(raw: &str) -> Result<Vec<String>> {
    let root: Value =
        serde_json::from_str(raw).map_err(|_| http::error("Malformed Google Beta answer"))?;
    root[0]
        .as_array()
        .ok_or_else(|| http::error("Missing Google Beta translations"))?
        .iter()
        .map(|answer| {
            answer
                .as_str()
                .map(unescape)
                .ok_or_else(|| http::error("Malformed Google Beta translation"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_lookup_prioritizes_the_translation_key_and_decodes_once() {
        let other = format!("AIza{}", "a".repeat(35));
        let correct = format!("AIza{}", "b".repeat(35));
        assert_eq!(
            keys(&format!("{other} translateHtml {correct}")).unwrap()[0],
            correct
        );
        assert_eq!(
            main_script(
                r#"https:\/\/translate.googleapis.com\/_\/translate_http\/_\/js\/k=x\\u003d1"#
            )
            .unwrap(),
            "https://translate.googleapis.com/_/translate_http/_/js/k=x=1"
        );
        assert_eq!(
            parse(r#"[["&lt;A&gt; &amp;copy; &#39; &#x1F600;"]]"#).unwrap(),
            ["<A> &copy; ' 😀"]
        );
    }
}
