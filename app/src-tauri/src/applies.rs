//! Classifies a config change.
use lt_core::config::Config;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    Live,
    Capture,
    Pipeline,
    Restart,
}

fn table(config: &Config, name: &str) -> Value {
    serde_json::to_value(config)
        .ok()
        .and_then(|mut value| value.get_mut(name).map(Value::take))
        .unwrap_or(Value::Null)
}

fn field(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

pub fn classify(old: &Config, new: &Config) -> Applies {
    let mut result = Applies::Live;
    let mut raise = |level: Applies, changed: bool| {
        if changed {
            result = result.max(level);
        }
    };
    let (capture_old, capture_new) = (table(old, "capture"), table(new, "capture"));
    for key in ["mode", "device", "apps"] {
        raise(
            Applies::Capture,
            field(&capture_old, key) != field(&capture_new, key),
        );
    }
    raise(
        Applies::Restart,
        field(&capture_old, "autostart") != field(&capture_new, "autostart"),
    );
    for name in [
        "audio",
        "vad",
        "asr",
        "filter",
        "routing",
        "join",
        "translate",
    ] {
        raise(Applies::Pipeline, table(old, name) != table(new, name));
    }
    let (transcript_old, transcript_new) = (table(old, "transcript"), table(new, "transcript"));
    raise(
        Applies::Restart,
        field(&transcript_old, "retention_days") != field(&transcript_new, "retention_days"),
    );
    raise(
        Applies::Restart,
        table(old, "logging") != table(new, "logging"),
    );
    let (models_old, models_new) = (table(old, "models"), table(new, "models"));
    raise(
        Applies::Restart,
        field(&models_old, "dir") != field(&models_new, "dir"),
    );
    result
}

/// Translator server settings need a new llama-server, not only a new pipeline.
pub fn server_changed(old: &Config, new: &Config) -> bool {
    let key = |config: &Config| {
        json!([
            config.translate.server_url,
            config.translate.threads,
            config.translate.ctx,
            config.translate.engine,
        ])
    };
    key(old) != key(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::config::CaptureApp;

    fn changed(edit: impl FnOnce(&mut Config)) -> Applies {
        let old = Config::default();
        let mut new = old.clone();
        edit(&mut new);
        classify(&old, &new)
    }

    #[test]
    fn each_table_maps_to_its_documented_level() {
        assert_eq!(changed(|_| {}), Applies::Live);
        assert_eq!(changed(|c| c.overlay.font_px = 30), Applies::Live);
        assert_eq!(changed(|c| c.hotkeys.pause = "".into()), Applies::Live);
        assert_eq!(changed(|c| c.transcript.enabled = false), Applies::Live);
        assert_eq!(
            changed(|c| c.models.source = "modelscope".into()),
            Applies::Live
        );
        assert_eq!(
            changed(|c| c.capture.mode = "apps".into()),
            Applies::Capture
        );
        assert_eq!(
            changed(|c| c.capture.apps = vec![CaptureApp {
                exe: "chrome.exe".into(),
                ..CaptureApp::default()
            }]),
            Applies::Capture
        );
        assert_eq!(changed(|c| c.vad.min_silence_s = 0.6), Applies::Pipeline);
        assert_eq!(
            changed(|c| c.routing.translate_other = vec!["ja".into()]),
            Applies::Pipeline
        );
        assert_eq!(changed(|c| c.translate.threads = 3), Applies::Pipeline);
        assert_eq!(changed(|c| c.capture.autostart = false), Applies::Restart);
        assert_eq!(
            changed(|c| c.logging.level = "debug".into()),
            Applies::Restart
        );
        assert_eq!(changed(|c| c.models.dir = "x".into()), Applies::Restart);
        // The most disruptive change wins.
        assert_eq!(
            changed(|c| {
                c.capture.mode = "apps".into();
                c.vad.threshold = 0.6;
            }),
            Applies::Pipeline
        );
    }

    #[test]
    fn only_server_settings_restart_the_translator_process() {
        let old = Config::default();
        let mut new = old.clone();
        new.translate.timeout_s = 20.0;
        assert!(!server_changed(&old, &new));
        new.translate.ctx = 2048;
        assert!(server_changed(&old, &new));
    }
}
