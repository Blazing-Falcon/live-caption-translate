use crate::http;
use lt_core::{
    engines::TranslationControl,
    error::{Error, Result},
    events::FailReason,
};
use regex::Regex;
use serde_json::Value;
use std::time::{Duration, Instant};

struct Credentials {
    key: String,
    token: String,
    ig: String,
    iid: String,
    expires: Instant,
}

#[derive(Default)]
pub struct Session {
    credentials: Option<Credentials>,
    sequence: u64,
}

impl Session {
    pub fn send(
        &mut self,
        agent: &ureq::Agent,
        texts: &[&str],
        control: &TranslationControl,
    ) -> Result<String> {
        if texts.len() != 1 {
            return Err(http::error("Bing accepts one text per request"));
        }
        for attempt in 0..2 {
            if self
                .credentials
                .as_ref()
                .is_none_or(|credentials| credentials.expires <= Instant::now())
            {
                let page = http::get(agent, "https://www.bing.com/translator", None, control)?;
                self.credentials = Some(credentials(&page)?);
            }
            let credentials = self.credentials.as_ref().expect("fetched above");
            self.sequence += 1;
            let url = format!(
                "https://www.bing.com/ttranslatev3?isVertical=1&IG={}&IID={}.{}",
                credentials.ig, credentials.iid, self.sequence
            );
            let body = format!(
                "fromLang=auto-detect&to=en&text={}&token={}&key={}",
                http::encode(texts[0]),
                http::encode(&credentials.token),
                http::encode(&credentials.key)
            );
            let response = agent
                .post(url)
                .config()
                .timeout_global(Some(http::timeout(control)?))
                .build()
                .header("Content-Type", "application/x-www-form-urlencoded")
                .send(body.as_bytes())
                .map_err(|error| http::map_error(error, control))?;
            let raw = http::read(response, control)?;
            let root: Value =
                serde_json::from_str(&raw).map_err(|_| http::error("Malformed Bing answer"))?;
            if root.is_object() {
                let status = root["statusCode"].as_u64().unwrap_or(0);
                self.credentials = None;
                if attempt == 0 {
                    continue;
                }
                return Err(Error::Translation {
                    reason: if status == 429 || status >= 500 {
                        FailReason::ServerUnavailable
                    } else {
                        FailReason::Error
                    },
                    message: format!("Bing refused the request ({status})"),
                });
            }
            return Ok(raw);
        }
        unreachable!()
    }
}

fn credentials(page: &str) -> Result<Credentials> {
    let pattern = Regex::new(
        r#"params_AbusePreventionHelper\s*=\s*\[\s*(\d+)\s*,\s*"([^"]+)"\s*,\s*(\d+)\s*\]"#,
    )
    .expect("fixed regex");
    let abuse = pattern
        .captures(page)
        .ok_or_else(|| http::error("Bing translator page has no credentials"))?;
    let capture = |pattern| {
        Regex::new(pattern)
            .expect("fixed regex")
            .captures(page)
            .map(|capture| capture[1].to_owned())
    };
    let lifetime =
        Duration::from_millis(abuse[3].parse::<u64>().unwrap_or(600_000).min(86_400_000));
    Ok(Credentials {
        key: abuse[1].to_owned(),
        token: abuse[2].to_owned(),
        ig: capture(r#"IG:"([0-9A-Fa-f]+)""#).unwrap_or(http::nonce()?),
        iid: capture(r#"data-iid="(translator\.\d+)""#).unwrap_or_else(|| "translator.5024".into()),
        expires: Instant::now() + lifetime - (lifetime / 2).min(Duration::from_secs(300)),
    })
}

pub fn parse(raw: &str) -> Result<Vec<String>> {
    let root: Value =
        serde_json::from_str(raw).map_err(|_| http::error("Malformed Bing answer"))?;
    root[0]["translations"][0]["text"]
        .as_str()
        .map(|text| vec![text.to_owned()])
        .ok_or_else(|| http::error("Missing Bing translation"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_credentials_and_http_success_refusals_are_distinct() {
        let credentials = credentials(r#"params_AbusePreventionHelper = [123,"test-token",3600000]; IG:"ABC123" data-iid="translator.5028""#).unwrap();
        assert_eq!(credentials.key, "123");
        assert_eq!(credentials.token, "test-token");
        assert_eq!(credentials.ig, "ABC123");
        assert_eq!(credentials.iid, "translator.5028");
        assert!(parse(r#"{"statusCode":400}"#).is_err());
    }
}
