use crate::http;
use lt_core::{engines::TranslationControl, error::Result};
use serde_json::{json, Value};

pub fn send(
    agent: &ureq::Agent,
    texts: &[&str],
    control: &TranslationControl,
) -> Result<(String, Vec<String>)> {
    let raw = request(agent, texts, control)?;
    let mut answers = parse(&raw, texts.len())?;
    for (position, text) in texts.iter().enumerate() {
        if answers[position].is_none() {
            let retry = request(agent, &[*text], control)?;
            answers[position] = parse(&retry, 1)?.remove(0);
        }
    }
    if answers.iter().any(Option::is_none) {
        return Err(http::error("Google alternate call was refused"));
    }
    Ok((raw, answers.into_iter().flatten().collect()))
}

fn request(agent: &ureq::Agent, texts: &[&str], control: &TranslationControl) -> Result<String> {
    let calls: Vec<_> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            json!([
                "MkEWBc",
                json!([[text, "auto", "en", 1], []]).to_string(),
                null,
                (i + 1).to_string()
            ])
        })
        .collect();
    let body = format!("f.req={}", http::encode(&json!([calls]).to_string()));
    let response = agent
        .post("https://translate.google.com/_/TranslateWebserverUi/data/batchexecute?rpcids=MkEWBc")
        .config()
        .timeout_global(Some(http::timeout(control)?))
        .build()
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(body.as_bytes())
        .map_err(|error| http::map_error(error, control))?;
    http::read(response, control)
}

pub fn parse(raw: &str, count: usize) -> Result<Vec<Option<String>>> {
    let start = raw
        .find('[')
        .ok_or_else(|| http::error("Malformed Google alternate answer"))?;
    let root: Value = serde_json::from_str(&raw[start..])
        .map_err(|_| http::error("Malformed Google alternate answer"))?;
    let entries = root
        .as_array()
        .ok_or_else(|| http::error("Malformed Google alternate answer"))?;
    let mut answers = vec![None; count];
    for entry in entries {
        if entry[0] != "wrb.fr" {
            continue;
        }
        let tag = entry[6].as_str().and_then(|tag| tag.parse::<usize>().ok());
        if tag.is_none() && !(count == 1 && entry[6] == "generic") {
            continue;
        }
        let Some(position) = tag.filter(|position| *position > 0 && *position <= count) else {
            if count == 1 && entry[6] == "generic" {
                if let Some(data) = entry[2].as_str() {
                    answers[0] = Some(read(data)?);
                }
                continue;
            }
            return Err(http::error("Google alternate answer has an unknown tag"));
        };
        if let Some(data) = entry[2].as_str() {
            if answers[position - 1].is_some() {
                return Err(http::error("Duplicate Google alternate answer tag"));
            }
            answers[position - 1] = Some(read(data)?);
        }
    }
    Ok(answers)
}

fn read(data: &str) -> Result<String> {
    let root: Value =
        serde_json::from_str(data).map_err(|_| http::error("Malformed Google alternate call"))?;
    let result = &root[1][0][0];
    if let Some(sentences) = result[5].as_array() {
        sentences
            .iter()
            .map(|sentence| {
                sentence[0]
                    .as_str()
                    .map(str::trim)
                    .ok_or_else(|| http::error("Malformed Google alternate sentence"))
            })
            .collect::<Result<Vec<_>>>()
            .map(|parts| parts.join(" "))
    } else {
        result[0]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| http::error("Malformed Google alternate text"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn answers_match_tags_and_refused_calls_remain_missing() {
        let data = |text| json!([null, [[[text]]]]).to_string();
        let body = json!([
            ["wrb.fr", "MkEWBc", data("second"), null, null, null, "2"],
            ["wrb.fr", "MkEWBc", null, null, null, null, "1"]
        ])
        .to_string();
        assert_eq!(parse(&body, 2).unwrap(), [None, Some("second".into())]);
        assert!(parse(&body, 1).is_err());
    }
}
