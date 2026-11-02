use lt_core::config::{
    AppTheme, CaptionCard, CaptionFont, CaptureConfig, Config, LogLevel, MessageLevel,
    OverlayConfig, OverlayStyle, PanelEdge,
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct TempConfig(PathBuf);
impl TempConfig {
    fn new() -> Self {
        let index = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("config-test-{}-{index}", std::process::id()));
        fs::create_dir_all(&directory).expect("create workspace test directory");
        Self(directory.join("config.toml"))
    }
}
impl Drop for TempConfig {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

fn warnings(messages: &[lt_core::config::Message]) -> Vec<&str> {
    messages
        .iter()
        .filter(|message| message.level == MessageLevel::Warn)
        .map(|message| message.text.as_str())
        .collect()
}

#[test]
fn typescript_defaults_round_trip_through_rust_json_without_missing_or_unknown_keys() {
    let source = include_str!("../../../app/ui/src/lib/settings.ts");
    let initializer = source
        .split_once("export const DEFAULT_CONFIG: Config = ")
        .expect("TypeScript default config initializer")
        .1
        .trim()
        .strip_suffix(';')
        .expect("initializer ends with semicolon");
    let value: serde_json::Value = serde_json::from_str(initializer).unwrap();
    let mut config: Config = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(config, Config::default());
    assert!(config.validate().is_empty());
    let encoded = serde_json::to_value(&config).unwrap();
    fn assert_matching_keys(left: &serde_json::Value, right: &serde_json::Value) {
        if let (Some(left), Some(right)) = (left.as_object(), right.as_object()) {
            assert_eq!(
                left.keys().collect::<Vec<_>>(),
                right.keys().collect::<Vec<_>>()
            );
            for (key, value) in left {
                assert_matching_keys(value, &right[key]);
            }
        }
    }
    assert_matching_keys(&value, &encoded);
    let decoded: Config = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, config);
}

#[test]
fn unknown_keys_are_dropped_and_named_in_one_info_message() {
    let (config, messages) = Config::from_toml(
        r#"
config_version = 2
future = { nested = { flag = true } }
[capture]
future_capture = "saved"
apps = [{exe="chrome.exe",name="Chrome",future_app={revision=7}}]
[audio]
future_audio = 2026-10-07T12:30:00Z
[translate]
future_translate = {deep={value=42}}
[models]
source = "modelscope"
dir = "D:/models"
[overlay.panel_rect]
monitor="display"
x=-123.25
y=40.5
w=960.0
h=140.0
future_rect={version=4}
"#,
    )
    .unwrap();
    assert!(warnings(&messages).is_empty(), "{messages:?}");
    let info: Vec<&str> = messages
        .iter()
        .filter(|message| message.level == MessageLevel::Info)
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(info.len(), 1, "{messages:?}");
    for path in [
        "future",
        "capture.future_capture",
        "capture.apps[0].future_app",
        "audio.future_audio",
        "translate.future_translate",
        "models.source",
        "overlay.panel_rect.future_rect",
    ] {
        assert!(info[0].contains(path), "{path} missing from {:?}", info[0]);
    }
    assert_eq!(config.overlay.panel_rect.as_ref().unwrap().x, -123.25);
    assert_eq!(config.capture.apps[0].name, "Chrome");
    assert_eq!(config.models.dir, "D:/models");
    let saved: toml::Value = toml::from_str(&config.to_toml().unwrap()).unwrap();
    assert!(saved.get("future").is_none());
    assert!(saved["audio"].get("future_audio").is_none());
    assert!(saved["models"].get("source").is_none());
    assert!(saved["overlay"]["panel_rect"].get("future_rect").is_none());
}

#[test]
fn invalid_types_and_choices_recover_each_key_without_discarding_neighbors() {
    let (config, messages) = Config::from_toml(
        r#"
config_version = "bad"
[capture]
mode = "microphone"
autostart = "yes"
device = "selected-device"
apps = [{exe=42,name="Named App",new_field=true}]
[audio]
normalize = 123
[asr]
threads = 2.5
[overlay]
style = "float"
panel_edge = "top"
panel_rect = false
[logging]
level = "verbose"
"#,
    )
    .unwrap();
    assert_eq!(config.config_version, 4);
    assert_eq!(config.capture.mode, CaptureConfig::default().mode);
    assert!(config.capture.autostart);
    assert_eq!(config.capture.device, "selected-device");
    assert_eq!(config.capture.apps[0].exe, "");
    assert_eq!(config.capture.apps[0].name, "Named App");
    assert!(config.audio.normalize);
    assert_eq!(config.asr.threads, 1);
    assert_eq!(config.overlay.style, OverlayStyle::Panel);
    assert_eq!(config.overlay.panel_edge, PanelEdge::Right);
    assert!(config.overlay.panel_rect.is_none());
    assert_eq!(config.logging.level, LogLevel::Info);
    let warnings = warnings(&messages);
    for key in [
        "config_version",
        "capture.mode",
        "capture.autostart",
        "capture.apps[0].exe",
        "audio.normalize",
        "asr.threads",
        "overlay.style",
        "overlay.panel_edge",
        "overlay.panel_rect",
        "logging.level",
    ] {
        assert!(
            warnings.iter().any(|message| message.starts_with(key)),
            "{key} missing from {warnings:?}"
        );
    }
    assert!(
        messages.iter().any(|message| {
            message.text
                == "dropped unknown settings: capture.apps[0].new_field, overlay.panel_rect"
        }),
        "{messages:?}"
    );
}

#[test]
fn every_bounded_number_clamps_to_the_limits_table() {
    let (config, messages) = Config::from_json(json!({
        "config_version":100,
        "asr":{"threads":-10,"pause_s":99},
        "translate":{"threads":99},
        "overlay":{"font_px":0,"background":99,"panel_lines":0,"flow_lines":11},
        "transcript":{"retention_days":-1},"logging":{"keep_files":-1}
    }))
    .unwrap();
    assert_eq!(config.config_version, 4);
    assert_eq!(config.asr.threads, 1);
    assert_eq!(config.asr.pause_s, 3.0);
    assert_eq!(config.translate.threads, 64);
    assert_eq!(config.overlay.font_px, 12);
    assert_eq!(config.overlay.background, 1.0);
    assert_eq!(config.overlay.panel_lines, 1);
    assert_eq!(config.overlay.flow_lines, 10);
    assert_eq!(config.transcript.retention_days, 0);
    assert_eq!(config.logging.keep_files, 0);
    assert_eq!(warnings(&messages).len(), 9, "{messages:?}");
}

#[test]
fn failed_patch_is_transactional_and_malformed_toml_is_reported() {
    let mut config = Config::default();
    let original = config.clone();
    assert!(config.merge_patch(json!([])).is_err());
    assert_eq!(config, original);
    assert!(config
        .merge_patch(json!({"overlay":{"font_px":30},"future":[null]}))
        .is_err());
    assert_eq!(config, original);
    assert!(Config::from_toml("[audio\nnormalize=true").is_err());
}

#[test]
fn endpoint_key_persists_but_debug_and_parse_errors_do_not_expose_it() {
    let mut config = Config::default();
    config.merge_patch(json!({"translate":{"engine":"openai","openai":{"base_url":"http://localhost:1234/v1","model":"test","api_key":"private-test-key","prompt":"Translate."}}})).unwrap();
    let saved = config.to_toml().unwrap();
    assert!(saved.contains("private-test-key"));
    assert_eq!(Config::from_toml(&saved).unwrap().0, config);
    let debug = format!("{config:?}");
    assert!(debug.contains("***"));
    assert!(!debug.contains("private-test-key"));
    let error = Config::from_toml("[translate.openai]\napi_key = \"private-test-key")
        .unwrap_err()
        .to_string();
    assert!(!error.contains("private-test-key"));
    assert!(error.contains("line 2"));
}

#[test]
fn failed_atomic_replace_preserves_existing_destination() {
    let temp = TempConfig::new();
    fs::create_dir(&temp.0).unwrap();
    fs::write(temp.0.join("marker"), "keep").unwrap();
    assert!(Config::default().save(&temp.0).is_err());
    assert_eq!(fs::read_to_string(temp.0.join("marker")).unwrap(), "keep");
    assert!(!temp.0.with_file_name("config.toml.tmp").exists());
}

/// A saved version 2 file keeps the values that still exist, loses the keys the app no longer
/// has, turns its second pass word into the switch, and is written as version 4 by the next save.
#[test]
fn a_version_2_file_loads_with_its_known_values() {
    let (config, messages) = Config::from_toml(
        r#"
config_version = 2
[capture]
mode = "apps"
apps = [{exe = "chrome.exe", name = "Chrome"}]
[asr]
threads = 3
pause_s = 1.2
second_pass = "off"
second_pass_min_logprob = -0.5
second_pass_threads = 3
[filter]
skip_music = true
fillers = ["呃"]
[translate]
threads = 3
temperature = 0.5
[overlay]
style = "flow"
font_px = 30
expire_s = 20.0
[logging]
level = "debug"
"#,
    )
    .unwrap();
    assert_eq!(config.config_version, 4);
    assert_eq!(config.capture.mode, lt_core::types::CaptureMode::Apps);
    assert_eq!(config.capture.apps.len(), 1);
    assert_eq!(config.asr.threads, 3);
    assert_eq!(config.asr.pause_s, 1.2);
    assert_eq!(config.asr.second_pass_threads, 3);
    assert_eq!(config.translate.threads, 3);
    assert_eq!(config.overlay.style, OverlayStyle::Flow);
    assert_eq!(config.overlay.font_px, 30);
    assert_eq!(config.logging.level, LogLevel::Debug);
    assert!(warnings(&messages).is_empty(), "{messages:?}");
    let saved: toml::Value = toml::from_str(&config.to_toml().unwrap()).unwrap();
    assert_eq!(saved["config_version"].as_integer(), Some(4));
    assert!(!config.asr.second_pass);
    assert_eq!(saved["asr"]["second_pass"].as_bool(), Some(false));
    assert!(saved["translate"].get("temperature").is_none());
    assert!(saved["overlay"].get("expire_s").is_none());
}

#[test]
fn a_version_1_file_takes_no_migration_path() {
    let (config, messages) = Config::from_toml(
        r#"
config_version = 1
[vad]
engine = "ten"
[asr]
engine = "xasr_stream"
threads = 3
stream_chunk_ms = 960
[translate]
threads = 0
[overlay]
style = "flow"
"#,
    )
    .unwrap();
    assert_eq!(config.config_version, 4);
    assert_eq!(config.asr.threads, 3);
    // A version 1 file wrote 0 for automatic threads, which is clamped like any other value.
    assert_eq!(config.translate.threads, 1);
    assert_eq!(config.overlay.style, OverlayStyle::Flow);
    assert!(warnings(&messages).len() == 1, "{messages:?}");
    let saved: toml::Value = toml::from_str(&config.to_toml().unwrap()).unwrap();
    assert_eq!(saved["config_version"].as_integer(), Some(4));
}

#[test]
fn a_removed_overlay_style_loads_as_panel() {
    let (config, messages) = Config::from_toml(
        r#"
config_version = 2
[overlay]
style = "bar"
"#,
    )
    .unwrap();
    assert_eq!(config.overlay.style, OverlayStyle::Panel);
    assert!(warnings(&messages).is_empty(), "{messages:?}");
    assert!(messages
        .iter()
        .any(|message| message.level == MessageLevel::Info));
    assert_eq!(config, Config::default());
}

#[test]
fn reset_restores_the_defaults_but_keeps_positions_apps_and_the_models_folder() {
    let (config, _) = Config::from_toml(
        r#"
config_version = 3
[capture]
mode = "apps"
apps = [{exe = "chrome.exe", name = "Chrome"}]
[translate]
threads = 2
[overlay]
style = "flow"
font_px = 34
panel_rect = {monitor = "A", x = 10, y = 20, w = 300, h = 400}
[models]
dir = "D:/models"
"#,
    )
    .unwrap();
    let reset = config.reset();
    let defaults = Config::default();
    assert_eq!(reset.capture.mode, defaults.capture.mode);
    assert_eq!(reset.translate.threads, defaults.translate.threads);
    assert_eq!(reset.overlay.style, OverlayConfig::default().style);
    assert_eq!(reset.overlay.font_px, defaults.overlay.font_px);
    assert_eq!(reset.capture.apps, config.capture.apps);
    assert_eq!(reset.overlay.panel_rect, config.overlay.panel_rect);
    assert_eq!(reset.models.dir, "D:/models");
}

/// A version 3 file gets the new settings at their defaults. A file that never chose a
/// translator stays on the local one, so an upgrade never sends text online.
#[test]
fn a_version_3_file_gains_the_new_settings_and_keeps_the_local_translator() {
    let (config, messages) = Config::from_toml(
        r#"
config_version = 3
[asr]
second_pass_threads = 3
[overlay]
font_px = 30
"#,
    )
    .unwrap();
    assert!(warnings(&messages).is_empty(), "{messages:?}");
    assert_eq!(config.config_version, 4);
    assert!(config.asr.second_pass);
    assert_eq!(config.asr.second_pass_threads, 3);
    assert!(config.translate.engine.is_local());
    assert_eq!(config.overlay.card, CaptionCard::Night);
    assert_eq!(config.overlay.font, CaptionFont::Rounded);
    assert_eq!(config.overlay.english_color, "#ffffff");
    assert_eq!(config.overlay.chinese_color, "#c9c9c4");
    assert_eq!(config.app.theme, AppTheme::System);
}

#[test]
fn a_caption_color_that_is_not_hex_falls_back_and_hex_is_kept_in_lowercase() {
    let (config, messages) = Config::from_toml(
        r##"
[overlay]
english_color = "yellow"
chinese_color = "#FFE27A"
card = "paper"
"##,
    )
    .unwrap();
    assert_eq!(config.overlay.english_color, "#ffffff");
    assert_eq!(config.overlay.chinese_color, "#ffe27a");
    assert_eq!(config.overlay.card, CaptionCard::Night);
    let warned = warnings(&messages);
    assert!(
        warned
            .iter()
            .any(|text| text.starts_with("overlay.english_color")),
        "{warned:?}"
    );
    assert!(
        warned.iter().any(|text| text.starts_with("overlay.card")),
        "{warned:?}"
    );
}
