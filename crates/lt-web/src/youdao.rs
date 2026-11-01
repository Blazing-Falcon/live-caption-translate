use crate::http;
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use base64::Engine;
use lt_core::{
    engines::TranslationControl,
    error::{Error, Result},
    events::FailReason,
};
use md5::{Digest, Md5};
use regex::Regex;
use serde_json::Value;

const PAGE: &str = "https://fanyi.youdao.com/";
struct Credentials {
    secret: String,
    key: [u8; 16],
    iv: [u8; 16],
}

#[derive(Default)]
pub struct Session {
    credentials: Option<Credentials>,
}

impl Session {
    pub fn send(
        &mut self,
        agent: &ureq::Agent,
        texts: &[&str],
        control: &TranslationControl,
    ) -> Result<(String, String)> {
        for attempt in 0..2 {
            if self.credentials.is_none() {
                self.credentials = Some(lookup(agent, control)?);
            }
            let credentials = self.credentials.as_ref().expect("looked up above");
            let text = texts
                .iter()
                .map(|text| text.lines().map(str::trim).collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join("\n");
            let mut fields = signed("webfanyi", &credentials.secret);
            fields.extend([
                ("i", text),
                ("from", "auto".into()),
                ("to", "en".into()),
                ("useTerm", "false".into()),
                ("dictResult", "true".into()),
            ]);
            let response = agent
                .post("https://dict.youdao.com/webtranslate")
                .config()
                .timeout_global(Some(http::timeout(control)?))
                .build()
                .header("Referer", PAGE)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .send(form(&fields).as_bytes())
                .map_err(|error| http::map_error(error, control))?;
            let encrypted = http::read(response, control)?;
            let decoded = decrypt(&encrypted, credentials);
            match decoded.and_then(|raw| {
                parse(&raw)?;
                Ok(raw)
            }) {
                Ok(raw) => return Ok((raw, encrypted)),
                Err(_) if attempt == 0 => {
                    control.check()?;
                    self.credentials = None;
                }
                Err(error) => {
                    self.credentials = None;
                    return Err(error);
                }
            }
        }
        unreachable!()
    }
}

fn lookup(agent: &ureq::Agent, control: &TranslationControl) -> Result<Credentials> {
    let page = http::get(agent, PAGE, Some(PAGE), control)?;
    let address = Regex::new(
        r#"https://shared\.ydstatic\.com/dict/translation-website/[^/"']+/js/app\.[^."']+\.js"#,
    )
    .expect("fixed regex")
    .find(&page)
    .map(|found| found.as_str())
    .ok_or_else(|| http::error("Youdao page no longer names its script"))?;
    let script = http::get(agent, address, Some(PAGE), control)?;
    let getter_pattern =
        Regex::new(r#""(webfanyi-key-getter[^"]*)",\w+="(\w+)""#).expect("fixed regex");
    let getter = getter_pattern
        .captures(&script)
        .ok_or_else(|| http::error("Youdao script has no key getter"))?;
    let raw = http::get(
        agent,
        &format!(
            "https://dict.youdao.com/webtranslate/key?{}",
            form(&signed(&getter[1], &getter[2]))
        ),
        Some(PAGE),
        control,
    )?;
    let root: Value =
        serde_json::from_str(&raw).map_err(|_| http::error("Malformed Youdao key answer"))?;
    if root["code"] != 0 {
        return Err(http::error("Youdao key request was refused"));
    }
    let capture = |pattern| {
        Regex::new(pattern)
            .expect("fixed regex")
            .captures(&script)
            .map(|capture| capture[1].to_owned())
    };
    let data = &root["data"];
    let key = data["aesKey"]
        .as_str()
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .or_else(|| capture(r#"decodeKey:"(.*?)","#))
        .ok_or_else(|| http::error("Youdao supplied no decode key"))?;
    let iv = data["aesIv"]
        .as_str()
        .filter(|iv| !iv.is_empty())
        .map(str::to_owned)
        .or_else(|| capture(r#"decodeIv:"(.*?)","#))
        .ok_or_else(|| http::error("Youdao supplied no decode IV"))?;
    let secret = data["secretKey"]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| http::error("Youdao supplied no signing key"))?;
    Ok(Credentials {
        secret: secret.into(),
        key: Md5::digest(key.as_bytes()).into(),
        iv: Md5::digest(iv.as_bytes()).into(),
    })
}

fn signed<'a>(id: &str, key: &str) -> Vec<(&'a str, String)> {
    let time = chrono::Utc::now().timestamp_millis().to_string();
    vec![
        ("keyid", id.into()),
        ("mysticTime", time.clone()),
        ("sign", sign(&time, key)),
        ("client", "fanyideskweb".into()),
        ("product", "webfanyi".into()),
        ("appVersion", "12.0.0".into()),
        ("vendor", "web".into()),
        ("keyfrom", "fanyi.web".into()),
        ("pointParam", "client,mysticTime,product".into()),
        ("yduuid", "abcdefg".into()),
        ("mid", "1".into()),
        ("screen", "1".into()),
        ("model", "1".into()),
        ("network", "wifi".into()),
        ("abtest", "0".into()),
    ]
}

fn sign(time: &str, key: &str) -> String {
    format!(
        "{:x}",
        Md5::digest(format!(
            "client=fanyideskweb&mysticTime={time}&product=webfanyi&key={key}"
        ))
    )
}

fn form(fields: &[(&str, String)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{key}={}", http::encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn decrypt(raw: &str, credentials: &Credentials) -> Result<String> {
    let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim().trim_end_matches('='))
        .map_err(|_| http::error("Invalid encrypted Youdao answer"))?;
    let decoded =
        cbc::Decryptor::<aes::Aes128>::new(&credentials.key.into(), &credentials.iv.into())
            .decrypt_padded_mut::<Pkcs7>(&mut bytes)
            .map_err(|_| http::error("Could not decode Youdao answer"))?;
    String::from_utf8(decoded.to_vec()).map_err(|_| http::error("Invalid Youdao answer encoding"))
}

pub fn parse(raw: &str) -> Result<Vec<String>> {
    let root: Value =
        serde_json::from_str(raw).map_err(|_| http::error("Malformed Youdao answer"))?;
    if root["code"] != 0 {
        let code = root["code"].as_i64().unwrap_or(-1);
        return Err(Error::Translation {
            reason: FailReason::Error,
            message: format!("Youdao refused the request ({code})"),
        });
    }
    root["translateResult"]
        .as_array()
        .ok_or_else(|| http::error("Missing Youdao translation lines"))?
        .iter()
        .map(|line| {
            let parts = line
                .as_array()
                .ok_or_else(|| http::error("Malformed Youdao translation line"))?;
            parts
                .iter()
                .map(|part| {
                    part["tgt"]
                        .as_str()
                        .ok_or_else(|| http::error("Missing Youdao translation segment"))
                })
                .collect::<Result<Vec<_>>>()
                .map(|parts| parts.concat().trim_end_matches(['\r', '\n']).to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;
    #[test]
    fn decrypts_url_safe_padded_transport_and_preserves_answer_lines() {
        let raw = br#"{"code":0,"translateResult":[[{"tgt":"First."},{"tgt":" Second."}],[{"tgt":"Third.\n"}]]}"#;
        let credentials = Credentials {
            secret: "test".into(),
            key: [1; 16],
            iv: [2; 16],
        };
        let mut buffer = [0; 256];
        buffer[..raw.len()].copy_from_slice(raw);
        let encoded =
            cbc::Encryptor::<aes::Aes128>::new(&credentials.key.into(), &credentials.iv.into())
                .encrypt_padded_mut::<Pkcs7>(&mut buffer, raw.len())
                .unwrap();
        let encoded = base64::engine::general_purpose::URL_SAFE.encode(encoded);
        let decoded = decrypt(&encoded, &credentials).unwrap();
        assert_eq!(parse(&decoded).unwrap(), ["First. Second.", "Third."]);
        assert!(decrypt("broken", &credentials).is_err());
    }
}
