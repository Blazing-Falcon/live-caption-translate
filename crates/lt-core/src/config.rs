//! Typed defaults, forgiving validation, and atomic persisted configuration.
//!
//! Unknown fields are retained at every table boundary for forward compatibility.
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub type ExtraFields = BTreeMap<String, toml::Value>;

macro_rules! config_table {
    ($name:ident { $($(#[$attribute:meta])* $field:ident: $ty:ty = $default:expr),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(default)]
        pub struct $name {
            $($(#[$attribute])* pub $field: $ty,)*
            #[serde(flatten)]
            pub extra: ExtraFields,
        }
        impl Default for $name {
            fn default() -> Self {
                Self { $($field: $default,)* extra: BTreeMap::new() }
            }
        }
    };
}

config_table!(Config {
    config_version: u32 = 1,
    capture: CaptureConfig = CaptureConfig::default(),
    audio: AudioConfig = AudioConfig::default(),
    vad: VadConfig = VadConfig::default(),
    asr: AsrConfig = AsrConfig::default(),
    filter: FilterConfig = FilterConfig::default(),
    routing: RoutingConfig = RoutingConfig::default(),
    join: JoinConfig = JoinConfig::default(),
    translate: TranslateConfig = TranslateConfig::default(),
    overlay: OverlayConfig = OverlayConfig::default(),
    latency: LatencyConfig = LatencyConfig::default(),
    hotkeys: HotkeysConfig = HotkeysConfig::default(),
    transcript: TranscriptConfig = TranscriptConfig::default(),
    logging: LoggingConfig = LoggingConfig::default(),
    models: ModelsConfig = ModelsConfig::default(),
});
config_table!(CaptureConfig {
    mode: String = "system".into(),
    device: String = "default".into(),
    apps: Vec<CaptureApp> = Vec::new(),
    autostart: bool = true,
});
config_table!(CaptureApp {
    exe: String = String::new(),
    name: String = String::new(),
});
config_table!(AudioConfig {
    normalize: bool = true,
    norm_attack_s: f32 = 1.0,
    norm_release_s: f32 = 5.0,
    norm_max_gain_db: f32 = 20.0,
    norm_gate_dbfs: f32 = -60.0,
});
config_table!(VadConfig {
    engine: String = "silero".into(),
    threshold: f32 = 0.5,
    min_speech_s: f32 = 0.25,
    min_silence_s: f32 = 0.4,
    pre_roll_s: f32 = 0.3,
    post_roll_s: f32 = 0.1,
    soft_cut_after_s: f32 = 7.0,
    soft_cut_prob: f32 = 0.35,
    hard_cut_s: f32 = 10.0,
});
config_table!(AsrConfig {
    engine: String = "sensevoice".into(),
    language: String = "auto".into(),
    use_itn: bool = true,
    threads: u32 = 1,
});
config_table!(FilterConfig {
    drop_music: bool = true,
    single_char_max_s: f32 = 0.6,
    single_char_allow: Vec<String> = ["对", "好", "是", "行", "不", "哦"].map(String::from).to_vec(),
    fillers: Vec<String> = ["呃", "嗯", "额", "uh", "um"].map(String::from).to_vec(),
});
config_table!(RoutingConfig {
    english: String = "passthrough".into(),
    translate_other: Vec<String> = Vec::new(),
});
config_table!(JoinConfig {
    hold_max_chars: u32 = 8,
    hold_window_s: f32 = 1.0,
    max_segments: u32 = 3,
    max_chars: u32 = 40,
});
config_table!(TranslateConfig {
    engine: String = "hymt2".into(),
    server_url: String = String::new(),
    threads: u32 = 0,
    ctx: u32 = 1024,
    temperature: f32 = 0.0,
    repeat_penalty: f32 = 1.05,
    max_tokens_cap: u32 = 256,
    timeout_s: f32 = 10.0,
    queue_join_max_chars: u32 = 120,
    skip_lag_s: f32 = 6.0,
    target: String = "en".into(),
});
config_table!(OverlayConfig {
    visible: bool = true,
    style: String = "bar".into(),
    font_px: u32 = 26,
    background: f32 = 0.82,
    show_source: bool = true,
    expire_s: f32 = 8.0,
    panel_edge: String = "right".into(),
    panel_lines: u32 = 5,
    live_source: bool = true,
    draft_display: String = "hold2".into(),
    #[serde(skip_serializing_if = "Option::is_none")]
    bar_rect: Option<OverlayRect> = None,
    #[serde(skip_serializing_if = "Option::is_none")]
    panel_rect: Option<OverlayRect> = None,
});
config_table!(LatencyConfig {
    mode: String = "auto".into(),
    decode_interval_s: f32 = 0.5,
    min_open_s: f32 = 1.0,
    comma_min_tokens: u32 = 8,
    stability: bool = true,
    split_long: bool = true,
    cap_tokens: u32 = 20,
    draft_context_s: f32 = 1.5,
    final_context_s: f32 = 10.0,
    draft_min_chars: u32 = 3,
    draft_grow_chars: u32 = 3,
    draft_keep_back_words: u32 = 2,
    draft_timeout_s: f32 = 3.0,
    draft_engine: String = "lmt60".into(),
    draft_server_url: String = String::new(),
    low_priority: bool = true,
    step_down: bool = true,
    step_down_cpu_pct: f32 = 80.0,
    step_up_cpu_pct: f32 = 65.0,
    step_down_lag_s: f32 = 3.0,
    auto_min_cores: u32 = 6,
});
config_table!(OverlayRect {
    monitor: String = String::new(),
    x: f64 = 0.0,
    y: f64 = 0.0,
    w: f64 = 960.0,
    h: f64 = 140.0,
});
config_table!(HotkeysConfig {
    move_lock: String = "Ctrl+Shift+L".into(),
    show_hide: String = "Ctrl+Shift+H".into(),
    pause: String = "Ctrl+Shift+P".into(),
});
config_table!(TranscriptConfig {
    enabled: bool = true,
    retention_days: u32 = 30,
});
config_table!(LoggingConfig {
    level: String = "info".into(),
    keep_files: u32 = 5,
});
config_table!(ModelsConfig {
    source: String = "huggingface".into(),
    dir: String = String::new(),
});

impl TranslateConfig {
    /// Threads for each llama-server: the user value clamped to 1 to 4, or 2 when automatic.
    pub fn server_threads(&self) -> u32 {
        if self.threads == 0 {
            2
        } else {
            self.threads.clamp(1, 4)
        }
    }
}

impl Config {
    /// A missing file is a first run and returns built-in defaults.
    pub fn load(path: impl AsRef<Path>) -> Result<(Self, Vec<String>)> {
        match fs::read_to_string(path.as_ref()) {
            Ok(contents) => Self::from_toml(&contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok((Self::default(), Vec::new()))
            }
            Err(error) => Err(Error::Io(error)),
        }
    }

    pub fn from_toml(contents: &str) -> Result<(Self, Vec<String>)> {
        let value = toml::from_str::<toml::Value>(contents).map_err(config_error)?;
        Self::from_value(value)
    }

    pub fn from_json(value: JsonValue) -> Result<(Self, Vec<String>)> {
        let mut config = Self::default();
        let messages = config.merge_patch(value)?;
        Ok((config, messages))
    }

    fn from_value(mut value: toml::Value) -> Result<(Self, Vec<String>)> {
        if !value.is_table() {
            return Err(Error::Config(
                "configuration must be an object/table".into(),
            ));
        }
        let mut messages = Vec::new();
        let schema = schema()?;
        normalize_types(&mut value, &schema, "", &mut messages);
        let mut config: Self = value.clone().try_into().map_err(config_error)?;
        // serde's flattened-field buffer erases TOML's native datetime type.
        // Recover unknown values from the parsed document before validation.
        config.restore_extra(&value, &schema);
        messages.extend(config.validate());
        for message in &messages {
            tracing::warn!(message = %message, "config validation");
        }
        Ok((config, messages))
    }

    /// Deep object merge; arrays replace wholesale and null resets a key.
    /// Failed patches leave this configuration untouched.
    pub fn merge_patch(&mut self, patch: JsonValue) -> Result<Vec<String>> {
        if !patch.is_object() {
            return Err(Error::Config(
                "configuration patch must be an object".into(),
            ));
        }
        // Document serialization recognizes TOML's datetime marker, unlike the
        // generic Value serializer used behind flattened serde fields.
        let mut value = toml::from_str::<toml::Value>(&self.to_toml()?).map_err(config_error)?;
        apply_patch(&mut value, patch)?;
        let (next, messages) = Self::from_value(value)?;
        *self = next;
        Ok(messages)
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(config_error)
    }

    /// Write and flush the sibling temporary file before replacing the target.
    /// std::fs::rename uses replacement semantics on Windows and Unix.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let contents = self.to_toml()?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut temporary = path.as_os_str().to_os_string();
        temporary.push(".tmp");
        let temporary = std::path::PathBuf::from(temporary);
        let write_result = (|| -> std::io::Result<()> {
            let mut file = File::create(&temporary)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result.map_err(Error::Io)
    }

    /// Clamp bounded numbers and replace invalid enum values with defaults.
    pub fn validate(&mut self) -> Vec<String> {
        let mut messages = Vec::new();
        let defaults = Self::default();
        macro_rules! number {
            ($table:ident, $field:ident, $min:expr, $max:expr) => {
                clamp_number(
                    &mut self.$table.$field,
                    $min,
                    $max,
                    defaults.$table.$field,
                    concat!(stringify!($table), ".", stringify!($field)),
                    &mut messages,
                );
            };
        }
        macro_rules! choice {
            ($table:ident, $field:ident, $values:expr) => {
                validate_choice(
                    &mut self.$table.$field,
                    $values,
                    &defaults.$table.$field,
                    concat!(stringify!($table), ".", stringify!($field)),
                    &mut messages,
                );
            };
        }
        clamp_number(
            &mut self.config_version,
            1,
            1,
            1,
            "config_version",
            &mut messages,
        );
        choice!(capture, mode, &["system", "apps"]);
        number!(audio, norm_attack_s, 0.1, 5.0);
        number!(audio, norm_release_s, 0.5, 20.0);
        number!(audio, norm_max_gain_db, 0.0, 30.0);
        number!(audio, norm_gate_dbfs, -90.0, -30.0);
        number!(vad, threshold, 0.2, 0.9);
        number!(vad, min_speech_s, 0.05, 1.0);
        number!(vad, min_silence_s, 0.15, 1.5);
        number!(vad, pre_roll_s, 0.0, 1.0);
        number!(vad, post_roll_s, 0.0, 0.5);
        number!(vad, hard_cut_s, 5.0, 20.0);
        let hard_cut = self.vad.hard_cut_s;
        number!(vad, soft_cut_after_s, 3.0, hard_cut);
        let threshold = self.vad.threshold;
        number!(vad, soft_cut_prob, 0.05, threshold);
        choice!(asr, language, &["auto", "zh", "en", "ja", "ko", "yue"]);
        number!(asr, threads, 1, 4);
        // No maximum is specified for this duration; it must remain finite.
        number!(filter, single_char_max_s, 0.0, f32::MAX);
        choice!(routing, english, &["passthrough"]);
        number!(join, hold_max_chars, 0, 20);
        number!(join, hold_window_s, 0.2, 2.0);
        number!(join, max_segments, 2, 5);
        number!(join, max_chars, 10, 80);
        number!(translate, threads, 0, 16);
        number!(translate, ctx, 512, 4096);
        number!(translate, temperature, 0.0, 1.0);
        number!(translate, repeat_penalty, 1.0, 1.3);
        number!(translate, max_tokens_cap, 32, 1024);
        number!(translate, timeout_s, 2.0, 60.0);
        number!(translate, queue_join_max_chars, 20, 300);
        number!(translate, skip_lag_s, 2.0, 30.0);
        choice!(translate, target, &["en"]);
        number!(overlay, font_px, 18, 40);
        number!(overlay, background, 0.0, 1.0);
        number!(overlay, expire_s, 3.0, 60.0);
        number!(overlay, panel_lines, 4, 6);
        number!(latency, decode_interval_s, 0.3, 2.0);
        number!(latency, min_open_s, 0.5, 3.0);
        number!(latency, comma_min_tokens, 2, 20);
        number!(latency, cap_tokens, 0, 60);
        if (1..10).contains(&self.latency.cap_tokens) {
            self.latency.cap_tokens = 10;
            messages.push(
                "latency.cap_tokens: values 1 to 9 raised to 10 (0 turns the cap off)".into(),
            );
        }
        number!(latency, draft_context_s, 0.0, 5.0);
        number!(latency, final_context_s, 0.0, 20.0);
        number!(latency, draft_min_chars, 1, 10);
        number!(latency, draft_grow_chars, 1, 20);
        number!(latency, draft_keep_back_words, 0, 5);
        number!(latency, draft_timeout_s, 1.0, 10.0);
        number!(latency, step_down_cpu_pct, 50.0, 100.0);
        let step_down_cpu = self.latency.step_down_cpu_pct;
        number!(latency, step_up_cpu_pct, 30.0, step_down_cpu);
        number!(latency, step_down_lag_s, 1.0, 10.0);
        number!(latency, auto_min_cores, 2, 64);
        choice!(latency, mode, &["auto", "continuous", "light", "off"]);
        choice!(overlay, draft_display, &["hold2", "settled", "all"]);
        choice!(overlay, style, &["bar", "panel"]);
        choice!(overlay, panel_edge, &["left", "right"]);
        choice!(logging, level, &["error", "warn", "info", "debug", "trace"]);
        choice!(models, source, &["huggingface", "modelscope"]);
        for (name, engine, default) in [
            ("vad.engine", &mut self.vad.engine, &defaults.vad.engine),
            ("asr.engine", &mut self.asr.engine, &defaults.asr.engine),
            (
                "translate.engine",
                &mut self.translate.engine,
                &defaults.translate.engine,
            ),
            (
                "latency.draft_engine",
                &mut self.latency.draft_engine,
                &defaults.latency.draft_engine,
            ),
        ] {
            if engine.trim().is_empty() {
                *engine = default.clone();
                messages.push(format!(
                    "{name}: empty engine name; using default {default:?}"
                ));
            }
        }
        for (name, rect) in [
            ("overlay.bar_rect", &mut self.overlay.bar_rect),
            ("overlay.panel_rect", &mut self.overlay.panel_rect),
        ] {
            if let Some(rect) = rect {
                let default = OverlayRect::default();
                clamp_number(
                    &mut rect.x,
                    f64::MIN,
                    f64::MAX,
                    default.x,
                    &format!("{name}.x"),
                    &mut messages,
                );
                clamp_number(
                    &mut rect.y,
                    f64::MIN,
                    f64::MAX,
                    default.y,
                    &format!("{name}.y"),
                    &mut messages,
                );
                clamp_number(
                    &mut rect.w,
                    0.0,
                    f64::MAX,
                    default.w,
                    &format!("{name}.w"),
                    &mut messages,
                );
                clamp_number(
                    &mut rect.h,
                    0.0,
                    f64::MAX,
                    default.h,
                    &format!("{name}.h"),
                    &mut messages,
                );
            }
        }
        self.unknown_messages(&mut messages);
        messages
    }

    fn unknown_messages(&self, messages: &mut Vec<String>) {
        unknown_fields(&self.extra, "", messages);
        macro_rules! table {
            ($($name:ident),* $(,)?) => { $(unknown_fields(&self.$name.extra, stringify!($name), messages);)* };
        }
        table!(
            capture, audio, vad, asr, filter, routing, join, translate, overlay, latency, hotkeys,
            transcript, logging, models
        );
        for (index, app) in self.capture.apps.iter().enumerate() {
            unknown_fields(&app.extra, &format!("capture.apps[{index}]"), messages);
        }
        if let Some(rect) = &self.overlay.bar_rect {
            unknown_fields(&rect.extra, "overlay.bar_rect", messages);
        }
        if let Some(rect) = &self.overlay.panel_rect {
            unknown_fields(&rect.extra, "overlay.panel_rect", messages);
        }
    }

    fn restore_extra(&mut self, value: &toml::Value, schema: &toml::Value) {
        self.extra = extract_unknown(value, schema);
        macro_rules! table {
            ($($name:ident),* $(,)?) => { $(
                if let (Some(value), Some(schema)) = (value.get(stringify!($name)), schema.get(stringify!($name))) {
                    self.$name.extra = extract_unknown(value, schema);
                }
            )* };
        }
        table!(
            capture, audio, vad, asr, filter, routing, join, translate, overlay, latency, hotkeys,
            transcript, logging, models
        );
        if let Some(apps) = value
            .get("capture")
            .and_then(|capture| capture.get("apps"))
            .and_then(toml::Value::as_array)
        {
            let schema = toml::Value::Table(toml::Table::from_iter([
                ("exe".into(), toml::Value::String(String::new())),
                ("name".into(), toml::Value::String(String::new())),
            ]));
            for (app, value) in self.capture.apps.iter_mut().zip(apps) {
                app.extra = extract_unknown(value, &schema);
            }
        }
        if let (Some(overlay), Some(schema)) = (value.get("overlay"), schema.get("overlay")) {
            for (key, rect) in [
                ("bar_rect", &mut self.overlay.bar_rect),
                ("panel_rect", &mut self.overlay.panel_rect),
            ] {
                if let (Some(rect), Some(value), Some(schema)) =
                    (rect, overlay.get(key), schema.get(key))
                {
                    rect.extra = extract_unknown(value, schema);
                }
            }
        }
    }
}

fn extract_unknown(value: &toml::Value, schema: &toml::Value) -> ExtraFields {
    match value.as_table() {
        Some(table) => table
            .iter()
            .filter(|(key, _)| schema.get(key.as_str()).is_none())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        None => BTreeMap::new(),
    }
}

fn config_error(error: impl std::fmt::Display) -> Error {
    Error::Config(error.to_string())
}

fn schema() -> Result<toml::Value> {
    let mut schema = toml::Value::try_from(Config::default()).map_err(config_error)?;
    let rect = toml::Value::try_from(OverlayRect::default()).map_err(config_error)?;
    if let Some(overlay) = schema
        .get_mut("overlay")
        .and_then(toml::Value::as_table_mut)
    {
        overlay.insert("bar_rect".into(), rect.clone());
        overlay.insert("panel_rect".into(), rect);
    }
    Ok(schema)
}

fn normalize_types(
    value: &mut toml::Value,
    default: &toml::Value,
    path: &str,
    messages: &mut Vec<String>,
) {
    if let (Some(table), Some(default_table)) = (value.as_table_mut(), default.as_table()) {
        for (key, field_default) in default_table {
            if path == "overlay"
                && matches!(key.as_str(), "bar_rect" | "panel_rect")
                && table.get(key).is_some_and(|field| !field.is_table())
            {
                table.remove(key);
                messages.push(format!(
                    "{path}.{key}: invalid type; using default placement"
                ));
                continue;
            }
            if let Some(field) = table.get_mut(key) {
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                normalize_types(field, field_default, &path, messages);
            }
        }
        return;
    }
    let valid = match (&*value, default) {
        (toml::Value::String(_), toml::Value::String(_))
        | (toml::Value::Boolean(_), toml::Value::Boolean(_)) => true,
        (toml::Value::Integer(number), toml::Value::Integer(_)) => {
            let (min, max) = integer_range(path);
            let clamped = (*number).clamp(min, max);
            if clamped != *number {
                messages.push(format!("{path}: {number} clamped to {clamped}"));
                *value = toml::Value::Integer(clamped);
            }
            true
        }
        (toml::Value::Integer(number), toml::Value::Float(_)) => {
            let number = *number as f64;
            normalize_float(value, number, path, messages)
        }
        (toml::Value::Float(number), toml::Value::Float(_)) => {
            let number = *number;
            normalize_float(value, number, path, messages)
        }
        (toml::Value::Array(items), toml::Value::Array(_)) => {
            if path == "capture.apps" {
                items.iter().all(toml::Value::is_table)
            } else {
                items.iter().all(toml::Value::is_str)
            }
        }
        _ => false,
    };
    if !valid {
        messages.push(format!(
            "{path}: invalid type or nonfinite value; using default"
        ));
        *value = default.clone();
        return;
    }
    if let Some(items) = value.as_array_mut().filter(|_| path == "capture.apps") {
        // CaptureApp's default consists solely of two empty strings.
        let app_schema = toml::Value::Table(toml::Table::from_iter([
            ("exe".into(), toml::Value::String(String::new())),
            ("name".into(), toml::Value::String(String::new())),
        ]));
        for (index, item) in items.iter_mut().enumerate() {
            normalize_types(item, &app_schema, &format!("{path}[{index}]"), messages);
        }
    }
}

fn normalize_float(
    value: &mut toml::Value,
    number: f64,
    path: &str,
    messages: &mut Vec<String>,
) -> bool {
    if !number.is_finite() {
        return false;
    }
    let (min, max) = match path {
        "audio.norm_attack_s" => (0.1, 5.0),
        "audio.norm_release_s" => (0.5, 20.0),
        "audio.norm_max_gain_db" => (0.0, 30.0),
        "audio.norm_gate_dbfs" => (-90.0, -30.0),
        "vad.threshold" => (0.2, 0.9),
        "vad.min_speech_s" => (0.05, 1.0),
        "vad.min_silence_s" => (0.15, 1.5),
        "vad.pre_roll_s" => (0.0, 1.0),
        "vad.post_roll_s" => (0.0, 0.5),
        "vad.hard_cut_s" => (5.0, 20.0),
        "vad.soft_cut_after_s" => (3.0, 20.0),
        "vad.soft_cut_prob" => (0.05, 0.9),
        "join.hold_window_s" => (0.2, 2.0),
        "latency.decode_interval_s" => (0.3, 2.0),
        "latency.min_open_s" => (0.5, 3.0),
        "latency.draft_context_s" => (0.0, 5.0),
        "latency.final_context_s" => (0.0, 20.0),
        "latency.draft_timeout_s" => (1.0, 10.0),
        "latency.step_down_cpu_pct" => (50.0, 100.0),
        "latency.step_up_cpu_pct" => (30.0, 100.0),
        "latency.step_down_lag_s" => (1.0, 10.0),
        "translate.temperature" | "overlay.background" => (0.0, 1.0),
        "translate.repeat_penalty" => (1.0, 1.3),
        "translate.timeout_s" => (2.0, 60.0),
        "translate.skip_lag_s" => (2.0, 30.0),
        "overlay.expire_s" => (3.0, 60.0),
        "filter.single_char_max_s" => (0.0, f64::from(f32::MAX)),
        _ => (f64::MIN, f64::MAX),
    };
    let clamped = number.clamp(min, max);
    if number != clamped {
        messages.push(format!("{path}: {number} clamped to {clamped}"));
    }
    *value = toml::Value::Float(clamped);
    true
}

fn integer_range(path: &str) -> (i64, i64) {
    match path {
        "config_version" => (1, 1),
        "asr.threads" => (1, 4),
        "join.hold_max_chars" => (0, 20),
        "join.max_segments" => (2, 5),
        "join.max_chars" => (10, 80),
        "translate.threads" => (0, 16),
        "translate.ctx" => (512, 4096),
        "translate.max_tokens_cap" => (32, 1024),
        "translate.queue_join_max_chars" => (20, 300),
        "overlay.font_px" => (18, 40),
        "overlay.panel_lines" => (4, 6),
        "latency.comma_min_tokens" => (2, 20),
        "latency.cap_tokens" => (0, 60),
        "latency.draft_min_chars" => (1, 10),
        "latency.draft_grow_chars" => (1, 20),
        "latency.draft_keep_back_words" => (0, 5),
        "latency.auto_min_cores" => (2, 64),
        _ => (0, i64::from(u32::MAX)),
    }
}

trait ConfigNumber: Copy + PartialOrd + PartialEq + std::fmt::Display {
    fn is_finite(self) -> bool;
}
impl ConfigNumber for u32 {
    fn is_finite(self) -> bool {
        true
    }
}
impl ConfigNumber for f32 {
    fn is_finite(self) -> bool {
        self.is_finite()
    }
}
impl ConfigNumber for f64 {
    fn is_finite(self) -> bool {
        self.is_finite()
    }
}
fn clamp_number<T: ConfigNumber>(
    value: &mut T,
    min: T,
    max: T,
    default: T,
    path: &str,
    messages: &mut Vec<String>,
) {
    let original = *value;
    if !value.is_finite() {
        *value = default;
        messages.push(format!("{path}: nonfinite value; using default {default}"));
    }
    if *value < min {
        *value = min;
    }
    if *value > max {
        *value = max;
    }
    if original.is_finite() && *value != original {
        messages.push(format!("{path}: {original} clamped to {value}"));
    }
}

fn validate_choice(
    value: &mut String,
    allowed: &[&str],
    default: &str,
    path: &str,
    messages: &mut Vec<String>,
) {
    if !allowed.contains(&value.as_str()) {
        messages.push(format!(
            "{path}: invalid value {value:?}; using default {default:?}"
        ));
        *value = default.into();
    }
}

fn unknown_fields(fields: &ExtraFields, path: &str, messages: &mut Vec<String>) {
    for key in fields.keys() {
        let path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        messages.push(format!("{path}: unknown key preserved"));
    }
}

fn apply_patch(value: &mut toml::Value, patch: JsonValue) -> Result<()> {
    match patch {
        JsonValue::Object(patch) => {
            if !value.is_table() {
                *value = toml::Value::Table(toml::Table::new());
            }
            let table = value
                .as_table_mut()
                .ok_or_else(|| Error::Config("patch table conversion failed".into()))?;
            for (key, patch) in patch {
                if patch.is_null() {
                    table.remove(&key);
                } else {
                    let field = table
                        .entry(key)
                        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                    apply_patch(field, patch)?;
                }
            }
        }
        JsonValue::Array(items) => {
            let mut array = Vec::with_capacity(items.len());
            for item in items {
                let mut value = toml::Value::Table(toml::Table::new());
                apply_patch(&mut value, item)?;
                array.push(value);
            }
            *value = toml::Value::Array(array);
        }
        JsonValue::String(string) => *value = toml::Value::String(string),
        JsonValue::Bool(boolean) => *value = toml::Value::Boolean(boolean),
        JsonValue::Number(number) => {
            *value = if let Some(integer) = number.as_i64() {
                toml::Value::Integer(integer)
            } else if let Some(integer) = number.as_u64() {
                // TOML integers are signed i64; bounded config fields clamp this next.
                toml::Value::Integer(i64::try_from(integer).unwrap_or(i64::MAX))
            } else {
                toml::Value::Float(
                    number
                        .as_f64()
                        .ok_or_else(|| Error::Config("unsupported JSON number".into()))?,
                )
            };
        }
        JsonValue::Null => {
            return Err(Error::Config(
                "null array values cannot be stored in TOML".into(),
            ))
        }
    }
    Ok(())
}
