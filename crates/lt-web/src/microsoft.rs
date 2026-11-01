use crate::http;
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use lt_core::{engines::TranslationControl, error::Result};
use serde_json::Value;
use sha2::Sha256;

pub const URL: &str = "api.cognitive.microsofttranslator.com/translate?api-version=3.0&to=en";
const KEY: [u8; 64] = [
    0xa2, 0x29, 0x3a, 0x3d, 0xd0, 0xdd, 0x32, 0x73, 0x97, 0x7a, 0x64, 0xdb, 0xc2, 0xf3, 0x27, 0xf5,
    0xd7, 0xbf, 0x87, 0xd9, 0x45, 0x9d, 0xf0, 0x5a, 0x09, 0x66, 0xc6, 0x30, 0xc6, 0x6a, 0xaa, 0x84,
    0x9a, 0x41, 0xaa, 0x94, 0x3a, 0xa8, 0xd5, 0x1a, 0x6e, 0x4d, 0xaa, 0xc9, 0xa3, 0x70, 0x12, 0x35,
    0xc7, 0xeb, 0x12, 0xf6, 0xe8, 0x23, 0x07, 0x9e, 0x47, 0x10, 0x95, 0x91, 0x88, 0x55, 0xd8, 0x17,
];

pub fn signature(url: &str, date: &str, nonce: &str) -> String {
    let message =
        format!("MSTranslatorAndroidApp{}{date}{nonce}", http::encode(url)).to_lowercase();
    let mut mac = Hmac::<Sha256>::new_from_slice(&KEY).expect("HMAC accepts a key of any size");
    mac.update(message.as_bytes());
    format!(
        "MSTranslatorAndroidApp::{}::{date}::{nonce}",
        STANDARD.encode(mac.finalize().into_bytes())
    )
}

pub fn send(
    agent: &ureq::Agent,
    texts: &[&str],
    control: &TranslationControl,
) -> Result<(String, Value)> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| http::error("Could not create request nonce"))?;
    let nonce: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let date = chrono::Utc::now()
        .format("%a, %d %b %Y %H:%M:%SGMT")
        .to_string();
    let signed = signature(URL, &date, &nonce);
    let body: Vec<Value> = texts
        .iter()
        .map(|text| serde_json::json!({"Text": text}))
        .collect();
    let response = agent
        .post(format!("https://{URL}"))
        .header("X-MT-Signature", &signed)
        .config()
        .timeout_global(Some(http::timeout(control)?))
        .build()
        .send_json(body)
        .map_err(|failure| http::map_error(failure, control))?;
    Ok((
        http::read(response, control)?,
        serde_json::json!({"url":URL,"date":date,"nonce":nonce,"signature":signed}),
    ))
}

pub fn parse(raw: &str) -> Result<Vec<String>> {
    let value: Value =
        serde_json::from_str(raw).map_err(|_| http::error("Malformed Microsoft response"))?;
    value
        .as_array()
        .ok_or_else(|| http::error("Expected Microsoft answer list"))?
        .iter()
        .map(|item| {
            item["translations"][0]["text"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| http::error("Missing Microsoft translation"))
        })
        .collect()
}
