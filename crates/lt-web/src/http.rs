use lt_core::{
    engines::TranslationControl,
    error::{Error, Result},
    events::FailReason,
};
use std::time::{Duration, Instant};

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(3)))
        .http_status_as_error(false)
        .build()
        .into()
}

pub fn timeout(control: &TranslationControl) -> Result<Duration> {
    control.check()?;
    Ok(control
        .deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_secs(8)))
}

pub fn error(message: impl Into<String>) -> Error {
    Error::Translation {
        reason: FailReason::Error,
        message: message.into(),
    }
}

pub fn map_error(failure: ureq::Error, control: &TranslationControl) -> Error {
    if let Err(error) = control.check() {
        return error;
    }
    Error::Translation {
        reason: if matches!(failure, ureq::Error::Timeout(_)) {
            FailReason::Timeout
        } else {
            FailReason::ServerUnavailable
        },
        message: "Translation endpoint request failed".into(),
    }
}

pub fn read(
    mut response: ureq::http::Response<ureq::Body>,
    control: &TranslationControl,
) -> Result<String> {
    control.check()?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(Error::Translation {
            reason: if status == 429 || status >= 500 {
                FailReason::ServerUnavailable
            } else {
                FailReason::Error
            },
            message: format!("Translation endpoint returned HTTP {status}"),
        });
    }
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|failure| map_error(failure, control))?;
    control.check()?;
    Ok(text)
}

pub fn encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").expect("write to string");
        }
    }
    encoded
}
