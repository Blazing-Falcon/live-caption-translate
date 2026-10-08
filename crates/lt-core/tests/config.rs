use lt_core::config::{Config, OverlayRect};
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
fn exact_defaults_round_trip_toml_and_json() {
    let config = Config::default();
    let serialized = config.to_toml().unwrap();
    let (reloaded, messages) = Config::from_toml(&serialized).unwrap();
    assert_eq!(reloaded, config);
    assert!(messages.is_empty(), "{messages:?}");
    let json_value = serde_json::to_value(&config).unwrap();
    assert_eq!(json_value.as_object().unwrap().len(), 15);
    assert_eq!(json_value["config_version"], 1);
    assert_eq!(
        json_value["capture"],
        json!({"mode":"system", "device":"default", "apps":[], "autostart":true})
    );
    assert_eq!(
        json_value["asr"],
        json!({"engine":"sensevoice", "language":"auto", "use_itn":true, "threads":1})
    );
    assert_eq!(
        json_value["filter"]["single_char_allow"],
        json!(["对", "好", "是", "行", "不", "哦"])
    );
    assert_eq!(
        json_value["filter"]["fillers"],
        json!(["呃", "嗯", "额", "uh", "um"])
    );
    assert_eq!(json_value["translate"]["threads"], 0);
    assert_eq!(json_value["overlay"]["font_px"], 26);
    assert_eq!(json_value["transcript"]["enabled"], true);
    assert_eq!(json_value["transcript"]["retention_days"], 30);
    assert_eq!(json_value["hotkeys"]["pause"], "Ctrl+Shift+P");
    assert!(json_value["overlay"].get("bar_rect").is_none());
    let decoded: Config = serde_json::from_value(json_value.clone()).unwrap();
    assert_eq!(decoded, config);
    let (decoded, messages) = Config::from_json(json_value).unwrap();
    assert_eq!(decoded, config);
    assert!(messages.is_empty(), "{messages:?}");
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
fn missing_fields_and_missing_file_use_defaults() {
    let temp = TempConfig::new();
    let (missing, messages) = Config::load(&temp.0).unwrap();
    assert_eq!(missing, Config::default());
    assert!(messages.is_empty());
    let (partial, messages) =
        Config::from_toml("[capture]\nautostart=false\n[translate]\nctx=2048").unwrap();
    assert!(!partial.capture.autostart);
    assert_eq!(partial.capture.mode, "system");
    assert_eq!(partial.translate.ctx, 2048);
    assert_eq!(partial.vad, Config::default().vad);
    assert!(messages.is_empty());
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
fn nonfinite_and_overflowing_float_values_are_safe() {
    let (config, messages) = Config::from_toml("[audio]\nnorm_attack_s=nan\nnorm_release_s=inf\n[translate]\ntemperature=-inf\ntimeout_s=1e300").unwrap();
    assert_eq!(config.audio.norm_attack_s, 1.0);
    assert_eq!(config.audio.norm_release_s, 5.0);
    assert_eq!(config.translate.temperature, 0.0);
    assert_eq!(config.translate.timeout_s, 60.0);
    assert_eq!(messages.len(), 4);
    let mut direct = Config::default();
    direct.vad.hard_cut_s = f32::NAN;
    direct.vad.soft_cut_after_s = 19.0;
    direct.overlay.background = f32::INFINITY;
    direct.overlay.bar_rect = Some(OverlayRect {
        x: f64::NAN,
        ..OverlayRect::default()
    });
    assert_eq!(direct.validate().len(), 4);
    assert_eq!(direct.vad.hard_cut_s, 10.0);
    assert_eq!(direct.vad.soft_cut_after_s, 10.0);
    assert_eq!(direct.overlay.background, 0.82);
    assert_eq!(direct.overlay.bar_rect.unwrap().x, 0.0);
}

#[test]
fn deep_patches_replace_arrays_and_clear_optional_positions() {
    let mut config = Config::default();
    config.merge_patch(json!({"capture":{"apps":[{"exe":"one.exe","name":"One"}]},"overlay":{"bar_rect":{"monitor":"display","x":1.25,"y":2.5,"w":800.0,"h":100.0}},"future":{"nested":{"one":1,"two":2}}})).unwrap();
    config.merge_patch(json!({"overlay":{"font_px":30,"bar_rect":{"x":99.5}},"capture":{"apps":[{"exe":"two.exe","name":"Two"}]},"future":{"nested":{"one":null,"three":3}}})).unwrap();
    let rect = config.overlay.bar_rect.as_ref().unwrap();
    assert_eq!(rect.x, 99.5);
    assert_eq!(rect.y, 2.5);
    assert_eq!(rect.monitor, "display");
    assert_eq!(config.overlay.font_px, 30);
    assert_eq!(config.capture.apps.len(), 1);
    assert_eq!(config.capture.apps[0].exe, "two.exe");
    assert!(config.extra["future"]["nested"].get("one").is_none());
    assert_eq!(
        config.extra["future"]["nested"]["two"].as_integer(),
        Some(2)
    );
    assert_eq!(
        config.extra["future"]["nested"]["three"].as_integer(),
        Some(3)
    );
    config
        .merge_patch(json!({"overlay":{"bar_rect":null,"font_px":null}}))
        .unwrap();
    assert!(config.overlay.bar_rect.is_none());
    assert_eq!(config.overlay.font_px, 26);
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
fn repeated_atomic_saves_replace_existing_file_and_leave_no_temporary_file() {
    let temp = TempConfig::new();
    let mut config = Config::default();
    config.save(&temp.0).unwrap();
    for font in [18, 40, 26] {
        config.overlay.font_px = font;
        config.save(&temp.0).unwrap();
        let (reloaded, messages) = Config::load(&temp.0).unwrap();
        assert_eq!(reloaded, config);
        assert!(messages.is_empty());
        assert!(!temp.0.with_file_name("config.toml.tmp").exists());
    }
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

#[test]
fn latency_defaults_match_the_v2_spec() {
    let latency = Config::default().latency;
    assert_eq!(latency.mode, "auto");
    assert_eq!(latency.decode_interval_s, 0.5);
    assert_eq!(latency.min_open_s, 1.0);
    assert_eq!(latency.comma_min_tokens, 8);
    assert!(latency.stability && latency.split_long && latency.low_priority && latency.step_down);
    assert_eq!(latency.cap_tokens, 20);
    assert_eq!(latency.draft_context_s, 1.5);
    assert_eq!(latency.final_context_s, 10.0);
    assert_eq!(
        (
            latency.draft_min_chars,
            latency.draft_grow_chars,
            latency.draft_keep_back_words
        ),
        (3, 3, 2)
    );
    assert_eq!(latency.draft_timeout_s, 3.0);
    assert_eq!(latency.draft_engine, "lmt60");
    assert_eq!(latency.draft_server_url, "");
    assert_eq!(
        (
            latency.step_down_cpu_pct,
            latency.step_up_cpu_pct,
            latency.step_down_lag_s
        ),
        (80.0, 65.0, 3.0)
    );
    assert_eq!(latency.auto_min_cores, 6);
    let overlay = Config::default().overlay;
    assert!(overlay.live_source);
    assert_eq!(overlay.draft_display, "hold2");
}

#[test]
fn latency_values_clamp_and_invalid_enums_fall_back() {
    let (config, messages) = Config::from_toml(
        r#"
[latency]
mode = "turbo"
decode_interval_s = 0.01
min_open_s = 99
comma_min_tokens = 1
cap_tokens = 5
draft_context_s = -1
final_context_s = 100
draft_min_chars = 0
draft_grow_chars = 99
draft_keep_back_words = 9
draft_timeout_s = 50
step_down_cpu_pct = 20
step_up_cpu_pct = 99
step_down_lag_s = 0.1
auto_min_cores = 1000
draft_engine = ""
[overlay]
draft_display = "sparkle"
"#,
    )
    .unwrap();
    let latency = &config.latency;
    assert_eq!(latency.mode, "auto");
    assert_eq!(latency.decode_interval_s, 0.3);
    assert_eq!(latency.min_open_s, 3.0);
    assert_eq!(latency.comma_min_tokens, 2);
    assert_eq!(latency.cap_tokens, 10);
    assert_eq!(latency.draft_context_s, 0.0);
    assert_eq!(latency.final_context_s, 20.0);
    assert_eq!(latency.draft_min_chars, 1);
    assert_eq!(latency.draft_grow_chars, 20);
    assert_eq!(latency.draft_keep_back_words, 5);
    assert_eq!(latency.draft_timeout_s, 10.0);
    assert_eq!(latency.step_down_cpu_pct, 50.0);
    assert_eq!(latency.step_up_cpu_pct, 50.0, "step-up follows step-down");
    assert_eq!(latency.step_down_lag_s, 1.0);
    assert_eq!(latency.auto_min_cores, 64);
    assert_eq!(latency.draft_engine, "lmt60");
    assert_eq!(config.overlay.draft_display, "hold2");
    assert!(messages.iter().any(|m| m.contains("latency.mode")));
    assert!(messages.iter().any(|m| m.contains("overlay.draft_display")));
}

#[test]
fn cap_tokens_zero_stays_zero_and_valid_values_pass() {
    for (input, expected) in [(0, 0), (10, 10), (60, 60), (61, 60), (9, 10)] {
        let (config, _) = Config::from_toml(&format!("[latency]\ncap_tokens = {input}")).unwrap();
        assert_eq!(config.latency.cap_tokens, expected, "input {input}");
    }
}

#[test]
fn a_v1_config_file_loads_unchanged_with_v2_defaults() {
    let v1 = r#"
config_version = 1
[capture]
mode = "apps"
[translate]
threads = 3
[overlay]
font_px = 30
show_source = false
"#;
    let (config, messages) = Config::from_toml(v1).unwrap();
    assert!(messages.is_empty(), "{messages:?}");
    assert_eq!(config.capture.mode, "apps");
    assert_eq!(config.translate.threads, 3);
    assert_eq!(config.overlay.font_px, 30);
    assert!(!config.overlay.show_source);
    assert_eq!(config.latency, Config::default().latency);
    assert!(config.overlay.live_source);
}

#[test]
fn translate_threads_zero_resolves_to_two_for_each_server() {
    let mut config = Config::default();
    assert_eq!(config.translate.server_threads(), 2);
    config.translate.threads = 3;
    assert_eq!(config.translate.server_threads(), 3);
    config.translate.threads = 16;
    assert_eq!(config.translate.server_threads(), 4);
}

#[test]
fn latency_json_patches_round_trip_and_keep_unknown_keys() {
    let (mut config, _) =
        Config::from_toml("[latency]\nfuture_latency = \"keep\"\nmode = \"light\"").unwrap();
    assert_eq!(config.latency.mode, "light");
    let messages = config
        .merge_patch(
            json!({"latency": {"mode": "continuous", "split_long": false, "cap_tokens": 30},
                            "overlay": {"live_source": false, "draft_display": "settled"}}),
        )
        .unwrap();
    assert!(
        messages.iter().all(|m| m.contains("unknown key preserved")),
        "{messages:?}"
    );
    assert_eq!(config.latency.mode, "continuous");
    assert!(!config.latency.split_long);
    assert_eq!(config.latency.cap_tokens, 30);
    assert!(!config.overlay.live_source);
    assert_eq!(config.overlay.draft_display, "settled");
    let toml = config.to_toml().unwrap();
    assert!(toml.contains("future_latency"));
    let (reloaded, _) = Config::from_toml(&toml).unwrap();
    assert_eq!(reloaded, config);
    let json_value = serde_json::to_value(&config).unwrap();
    let (from_json, _) = Config::from_json(json_value).unwrap();
    assert_eq!(from_json, config);
}
