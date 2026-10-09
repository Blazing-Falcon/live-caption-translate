use lt_core::config::Config;
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
fn unknown_values_survive_nested_patches_and_disk_round_trips() {
    let input = r#"
future = { nested = { flag = true, numbers = [1, 2, 3] } }
[capture]
future_capture = "saved"
apps = [{exe="chrome.exe",name="Chrome", future_app={revision=7}}]
[audio]
future_audio = 2026-10-07T12:30:00Z
[vad]
future_vad = 1
[asr]
future_asr = 2
[filter]
future_filter = "filter"
[routing]
future_routing = "routing"
[join]
future_join = false
[translate]
future_translate = {deep={value=42}}
[overlay]
future_overlay = ["value"]
[overlay.bar_rect]
monitor="display"
x=-123.25
y=40.5
w=960.0
h=140.0
future_rect={version=4}
[hotkeys]
future_hotkeys = true
[transcript]
future_transcript = 4
[logging]
future_logging = "log"
[models]
future_models = "model"
"#;
    let temp = TempConfig::new();
    let (mut config, messages) = Config::from_toml(input).unwrap();
    assert_eq!(messages.len(), 16, "{messages:?}");
    assert!(config.audio.extra["future_audio"].is_datetime());
    config.merge_patch(json!({"translate":{"ctx":2048,"future_translate":{"deep":{"other":true}}},"overlay":{"bar_rect":{"x":-222.5}}})).unwrap();
    config.save(&temp.0).unwrap();
    let (reloaded, _) = Config::load(&temp.0).unwrap();
    assert_eq!(config, reloaded);
    assert_eq!(
        reloaded.translate.extra["future_translate"]["deep"]["value"].as_integer(),
        Some(42)
    );
    assert_eq!(
        reloaded.translate.extra["future_translate"]["deep"]["other"].as_bool(),
        Some(true)
    );
    assert_eq!(reloaded.overlay.bar_rect.as_ref().unwrap().x, -222.5);
    assert_eq!(
        reloaded.overlay.bar_rect.as_ref().unwrap().extra["future_rect"]["version"].as_integer(),
        Some(4)
    );
    assert!(reloaded.audio.extra["future_audio"].is_datetime());
}

#[test]
fn invalid_types_and_enums_recover_each_key_without_discarding_neighbors() {
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
language = "es"
threads = 2.5
use_itn = false
[filter]
fillers = ["um", 4]
[routing]
english = "translate"
translate_other = [9]
[join]
hold_window_s = "one"
[translate]
target = "fr"
[overlay]
style = "float"
panel_edge = "top"
bar_rect = false
[logging]
level = "verbose"
[models]
source = "elsewhere"
"#,
    )
    .unwrap();
    assert_eq!(config.config_version, 1);
    assert_eq!(config.capture.mode, "system");
    assert!(config.capture.autostart);
    assert_eq!(config.capture.device, "selected-device");
    assert_eq!(config.capture.apps[0].exe, "");
    assert_eq!(config.capture.apps[0].name, "Named App");
    assert_eq!(
        config.capture.apps[0].extra["new_field"].as_bool(),
        Some(true)
    );
    assert!(config.audio.normalize);
    assert_eq!(config.asr.language, "auto");
    assert_eq!(config.asr.threads, 1);
    assert!(!config.asr.use_itn);
    assert_eq!(config.filter.fillers, Config::default().filter.fillers);
    assert_eq!(config.routing.english, "passthrough");
    assert!(config.routing.translate_other.is_empty());
    assert_eq!(config.join.hold_window_s, 1.0);
    assert_eq!(config.translate.target, "en");
    assert_eq!(config.overlay.style, "bar");
    assert_eq!(config.overlay.panel_edge, "right");
    assert!(config.overlay.bar_rect.is_none());
    assert_eq!(config.logging.level, "info");
    assert_eq!(config.models.source, "huggingface");
    assert_eq!(messages.len(), 18, "{messages:?}");
}

#[test]
fn every_bounded_numeric_setting_clamps_and_dependent_limits_follow_validated_values() {
    let (config, messages) = Config::from_json(json!({
        "config_version":100,
        "audio":{"norm_attack_s":-1,"norm_release_s":99,"norm_max_gain_db":99,"norm_gate_dbfs":-100},
        "vad":{"threshold":0.1,"min_speech_s":0,"min_silence_s":99,"pre_roll_s":-1,"post_roll_s":99,"hard_cut_s":4,"soft_cut_after_s":19,"soft_cut_prob":0.8},
        "asr":{"threads":-10},
        "join":{"hold_max_chars":99,"hold_window_s":0,"max_segments":99,"max_chars":0},
        "translate":{"threads":99,"ctx":0,"temperature":99,"repeat_penalty":0,"max_tokens_cap":99_999,"timeout_s":0,"queue_join_max_chars":0,"skip_lag_s":0},
        "overlay":{"font_px":0,"background":99,"expire_s":0,"panel_lines":0},
        "transcript":{"retention_days":-1},"logging":{"keep_files":-1}
    })).unwrap();
    assert_eq!(config.config_version, 1);
    assert_eq!(config.audio.norm_attack_s, 0.1);
    assert_eq!(config.audio.norm_release_s, 20.0);
    assert_eq!(config.audio.norm_max_gain_db, 30.0);
    assert_eq!(config.audio.norm_gate_dbfs, -90.0);
    assert_eq!(config.vad.threshold, 0.2);
    assert_eq!(config.vad.min_speech_s, 0.05);
    assert_eq!(config.vad.min_silence_s, 1.5);
    assert_eq!(config.vad.pre_roll_s, 0.0);
    assert_eq!(config.vad.post_roll_s, 0.5);
    assert_eq!(config.vad.hard_cut_s, 5.0);
    assert_eq!(config.vad.soft_cut_after_s, 5.0);
    assert_eq!(config.vad.soft_cut_prob, 0.2);
    assert_eq!(config.asr.threads, 1);
    assert_eq!(config.join.hold_max_chars, 20);
    assert_eq!(config.join.hold_window_s, 0.2);
    assert_eq!(config.join.max_segments, 5);
    assert_eq!(config.join.max_chars, 10);
    assert_eq!(config.translate.threads, 16);
    assert_eq!(config.translate.ctx, 512);
    assert_eq!(config.translate.temperature, 1.0);
    assert_eq!(config.translate.repeat_penalty, 1.0);
    assert_eq!(config.translate.max_tokens_cap, 1024);
    assert_eq!(config.translate.timeout_s, 2.0);
    assert_eq!(config.translate.queue_join_max_chars, 20);
    assert_eq!(config.translate.skip_lag_s, 2.0);
    assert_eq!(config.overlay.font_px, 18);
    assert_eq!(config.overlay.background, 1.0);
    assert_eq!(config.overlay.expire_s, 3.0);
    assert_eq!(config.overlay.panel_lines, 4);
    assert_eq!(config.transcript.retention_days, 0);
    assert_eq!(config.logging.keep_files, 0);
    assert_eq!(messages.len(), 32, "{messages:?}");
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
fn failed_atomic_replace_preserves_existing_destination() {
    let temp = TempConfig::new();
    fs::create_dir(&temp.0).unwrap();
    fs::write(temp.0.join("marker"), "keep").unwrap();
    assert!(Config::default().save(&temp.0).is_err());
    assert_eq!(fs::read_to_string(temp.0.join("marker")).unwrap(), "keep");
    assert!(!temp.0.with_file_name("config.toml.tmp").exists());
}
