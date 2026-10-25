//! Typed defaults, forgiving validation, and atomic persisted configuration.
//!
//! A wrong type or an unknown choice takes its default, and a number is clamped to its bounds.
//! Keys the app does not know are dropped on load and named in one message.
use crate::{
    error::{Error, Result},
    types::CaptureMode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// The version this build reads and writes. An older file loads as it is and is saved as this.
pub const CONFIG_VERSION: u32 = 3;

/// The most threads a setting accepts; engines also stay within the PC's logical CPUs.
pub const MAX_THREADS: u32 = 64;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OverlayStyle {
    #[default]
    Panel,
    Flow,
}

impl OverlayStyle {
    const NAMES: &'static [&'static str] = &["panel", "flow"];
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelEdge {
    Left,
    #[default]
    Right,
}

impl PanelEdge {
    const NAMES: &'static [&'static str] = &["left", "right"];
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    const NAMES: &'static [&'static str] = &["error", "warn", "info", "debug", "trace"];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a load or a patch went: `Info` names a setting the app dropped, `Warn` a value it fixed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageLevel {
    Info,
    Warn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub level: MessageLevel,
    pub text: String,
}

impl Message {
    fn info(text: String) -> Self {
        Self {
            level: MessageLevel::Info,
            text,
        }
    }

    fn warn(text: String) -> Self {
        Self {
            level: MessageLevel::Warn,
            text,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub config_version: u32,
    pub capture: CaptureConfig,
    pub audio: AudioConfig,
    pub asr: AsrConfig,
    pub translate: TranslateConfig,
    pub overlay: OverlayConfig,
    pub performance: PerformanceConfig,
    pub hotkeys: HotkeysConfig,
    pub transcript: TranscriptConfig,
    pub logging: LoggingConfig,
    pub models: ModelsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            capture: CaptureConfig::default(),
            audio: AudioConfig::default(),
            asr: AsrConfig::default(),
            translate: TranslateConfig::default(),
            overlay: OverlayConfig::default(),
            performance: PerformanceConfig::default(),
            hotkeys: HotkeysConfig::default(),
            transcript: TranscriptConfig::default(),
            logging: LoggingConfig::default(),
            models: ModelsConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub mode: CaptureMode,
    pub device: String,
    pub apps: Vec<CaptureApp>,
    pub autostart: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            mode: CaptureMode::System,
            device: "default".into(),
            apps: Vec::new(),
            autostart: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureApp {
    pub exe: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub normalize: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { normalize: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    pub threads: u32,
    pub pause_s: f32,
    pub second_pass_threads: u32,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            threads: 1,
            pause_s: 0.8,
            second_pass_threads: 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranslateConfig {
    pub threads: u32,
}

impl Default for TranslateConfig {
    fn default() -> Self {
        Self { threads: 4 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    pub visible: bool,
    pub style: OverlayStyle,
    pub font_px: u32,
    pub background: f32,
    pub show_source: bool,
    pub panel_edge: PanelEdge,
    pub panel_lines: u32,
    pub flow_lines: u32,
    pub live_source: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel_rect: Option<OverlayRect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow_rect: Option<OverlayRect>,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            visible: true,
            style: OverlayStyle::Panel,
            font_px: 26,
            background: 0.82,
            show_source: true,
            panel_edge: PanelEdge::Right,
            panel_lines: 5,
            flow_lines: 3,
            live_source: true,
            panel_rect: None,
            flow_rect: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayRect {
    pub monitor: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Default for OverlayRect {
    fn default() -> Self {
        Self {
            monitor: String::new(),
            x: 0.0,
            y: 0.0,
            w: 960.0,
            h: 140.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceConfig {
    pub low_priority: bool,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self { low_priority: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeysConfig {
    pub move_lock: String,
    pub show_hide: String,
    pub pause: String,
    /// The overlay lets clicks through, so without a key the control window is reachable only
    /// from the tray.
    pub settings: String,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        Self {
            move_lock: "Ctrl+Shift+L".into(),
            show_hide: "Ctrl+Shift+H".into(),
            pause: "Ctrl+Shift+P".into(),
            settings: "Ctrl+Shift+S".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptConfig {
    pub enabled: bool,
    pub retention_days: u32,
}

impl Default for TranscriptConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 30,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: LogLevel,
    pub keep_files: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            keep_files: 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelsConfig {
    pub source: String,
    pub dir: String,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            source: "huggingface".into(),
            dir: String::new(),
        }
    }
}

/// The logical CPUs of this PC: the most threads an engine gets.
pub fn logical_cpus() -> u32 {
    std::thread::available_parallelism()
        .map_or(1, |count| u32::try_from(count.get()).unwrap_or(u32::MAX))
}

impl AsrConfig {
    /// Recognizer threads, at most the logical CPUs of this PC.
    pub fn engine_threads(&self) -> u32 {
        self.threads.clamp(1, logical_cpus())
    }

    /// Second-pass recognizer threads, at most the logical CPUs of this PC.
    pub fn second_pass_engine_threads(&self) -> u32 {
        self.second_pass_threads.clamp(1, logical_cpus())
    }
}

impl TranslateConfig {
    /// llama-server threads, at most the logical CPUs of this PC.
    pub fn server_threads(&self) -> u32 {
        self.threads.clamp(1, logical_cpus())
    }

    /// The manifest id and file name of the translation model.
    pub const MODEL: (&'static str, &'static str) = (
        "hy-mt2-1.8b-ternary-tq2_0",
        "Hy-MT2-1.8B-ternary-TQ2_0.gguf",
    );
}

/// Every bounded number and its bounds. The TOML pre-pass and `validate` both read this list.
macro_rules! limits {
    ($($table:ident.$field:ident => $min:expr, $max:expr;)*) => {
        const LIMITS: &[(&str, f64, f64)] = &[
            $((concat!(stringify!($table), ".", stringify!($field)), $min as f64, $max as f64)),*
        ];

        fn clamp_all(config: &mut Config, messages: &mut Vec<String>) {
            let defaults = Config::default();
            $(
                clamp_number(
                    &mut config.$table.$field,
                    $min,
                    $max,
                    defaults.$table.$field,
                    concat!(stringify!($table), ".", stringify!($field)),
                    messages,
                );
            )*
        }
    };
}

limits! {
    asr.threads => 1, MAX_THREADS;
    asr.pause_s => 0.3, 3.0;
    asr.second_pass_threads => 1, MAX_THREADS;
    translate.threads => 1, MAX_THREADS;
    overlay.font_px => 12, 40;
    overlay.background => 0.0, 1.0;
    overlay.panel_lines => 1, 10;
    overlay.flow_lines => 1, 10;
    transcript.retention_days => 0, u32::MAX;
    logging.keep_files => 0, u32::MAX;
}

/// A saved choice that the app now spells differently.
const VALUE_MIGRATIONS: &[(&str, &str, &str)] = &[("overlay.style", "bar", "panel")];

const MODEL_SOURCES: &[&str] = &["huggingface", "modelscope"];

fn choices(path: &str) -> Option<&'static [&'static str]> {
    Some(match path {
        "capture.mode" => CaptureMode::NAMES,
        "overlay.style" => OverlayStyle::NAMES,
        "overlay.panel_edge" => PanelEdge::NAMES,
        "logging.level" => LogLevel::NAMES,
        "models.source" => MODEL_SOURCES,
        _ => return None,
    })
}

fn limits(path: &str) -> Option<(f64, f64)> {
    LIMITS
        .iter()
        .find(|(name, _, _)| *name == path)
        .map(|(_, min, max)| (*min, *max))
}

impl Config {
    /// A missing file is a first run and returns built-in defaults.
    pub fn load(path: impl AsRef<Path>) -> Result<(Self, Vec<Message>)> {
        match fs::read_to_string(path.as_ref()) {
            Ok(contents) => Self::from_toml(&contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok((Self::default(), Vec::new()))
            }
            Err(error) => Err(Error::Io(error)),
        }
    }

    pub fn from_toml(contents: &str) -> Result<(Self, Vec<Message>)> {
        let value = toml::from_str::<toml::Value>(contents).map_err(config_error)?;
        Self::from_value(value)
    }

    pub fn from_json(value: JsonValue) -> Result<(Self, Vec<Message>)> {
        let mut config = Self::default();
        let messages = config.merge_patch(value)?;
        Ok((config, messages))
    }

    fn from_value(mut value: toml::Value) -> Result<(Self, Vec<Message>)> {
        let Some(table) = value.as_table_mut() else {
            return Err(Error::Config(
                "configuration must be an object/table".into(),
            ));
        };
        let mut messages = Vec::new();
        let mut dropped = Vec::new();
        if table
            .remove("config_version")
            .is_some_and(|version| !version.is_integer())
        {
            messages.push(Message::warn(format!(
                "config_version: invalid type; using {CONFIG_VERSION}"
            )));
        }
        let schema = schema()?;
        if let Some(schema) = schema.as_table() {
            normalize_table(table, schema, "", &mut messages, &mut dropped);
        }
        if !dropped.is_empty() {
            messages.push(Message::info(format!(
                "dropped unknown settings: {}",
                dropped.join(", ")
            )));
        }
        table.insert(
            "config_version".into(),
            toml::Value::Integer(i64::from(CONFIG_VERSION)),
        );
        let mut config: Self = value.try_into().map_err(config_error)?;
        messages.extend(config.validate().into_iter().map(Message::warn));
        Ok((config, messages))
    }

    /// Deep object merge; arrays replace wholesale and null resets a key.
    /// Failed patches leave this configuration untouched.
    pub fn merge_patch(&mut self, patch: JsonValue) -> Result<Vec<Message>> {
        if !patch.is_object() {
            return Err(Error::Config(
                "configuration patch must be an object".into(),
            ));
        }
        let mut value = toml::from_str::<toml::Value>(&self.to_toml()?).map_err(config_error)?;
        apply_patch(&mut value, patch)?;
        let (next, messages) = Self::from_value(value)?;
        *self = next;
        Ok(messages)
    }

    /// The defaults, keeping what the user placed or picked rather than tuned: the overlay's
    /// positions, the apps chosen for capture and the models folder.
    pub fn reset(&self) -> Self {
        let mut config = Self::default();
        config.overlay.panel_rect = self.overlay.panel_rect.clone();
        config.overlay.flow_rect = self.overlay.flow_rect.clone();
        config.capture.apps = self.capture.apps.clone();
        config.models.dir = self.models.dir.clone();
        config
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

    /// Clamp bounded numbers and the saved overlay rects.
    pub fn validate(&mut self) -> Vec<String> {
        let mut messages = Vec::new();
        clamp_all(self, &mut messages);
        for (name, rect) in [
            ("overlay.panel_rect", &mut self.overlay.panel_rect),
            ("overlay.flow_rect", &mut self.overlay.flow_rect),
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
        messages
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
        overlay.insert("panel_rect".into(), rect.clone());
        overlay.insert("flow_rect".into(), rect);
    }
    Ok(schema)
}

fn capture_app_schema() -> toml::Table {
    toml::Table::from_iter([
        ("exe".into(), toml::Value::String(String::new())),
        ("name".into(), toml::Value::String(String::new())),
    ])
}

/// Replace every value in `table` that does not fit its default, and drop keys the schema lacks.
fn normalize_table(
    table: &mut toml::Table,
    schema: &toml::Table,
    path: &str,
    messages: &mut Vec<Message>,
    dropped: &mut Vec<String>,
) {
    let mut unknown = Vec::new();
    for (key, value) in table.iter_mut() {
        let child = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        let Some(default) = schema.get(key) else {
            unknown.push(key.clone());
            continue;
        };
        if matches!(child.as_str(), "overlay.panel_rect" | "overlay.flow_rect") && !value.is_table()
        {
            messages.push(Message::warn(format!(
                "{child}: invalid type; using default placement"
            )));
            unknown.push(key.clone());
            continue;
        }
        normalize_value(value, default, &child, messages, dropped);
    }
    for key in unknown {
        table.remove(&key);
        dropped.push(if path.is_empty() {
            key
        } else {
            format!("{path}.{key}")
        });
    }
}

fn normalize_value(
    value: &mut toml::Value,
    default: &toml::Value,
    path: &str,
    messages: &mut Vec<Message>,
    dropped: &mut Vec<String>,
) {
    match default {
        toml::Value::Table(default_table) => {
            if let Some(table) = value.as_table_mut() {
                normalize_table(table, default_table, path, messages, dropped);
            } else {
                *value = default.clone();
                messages.push(Message::warn(format!(
                    "{path}: invalid type; using default"
                )));
            }
        }
        toml::Value::Array(_) => {
            let valid = value
                .as_array()
                .is_some_and(|items| items.iter().all(toml::Value::is_table));
            if !valid {
                *value = default.clone();
                messages.push(Message::warn(format!(
                    "{path}: invalid type; using default"
                )));
                return;
            }
            let app_schema = capture_app_schema();
            if let Some(items) = value.as_array_mut() {
                for (index, item) in items.iter_mut().enumerate() {
                    if let Some(table) = item.as_table_mut() {
                        normalize_table(
                            table,
                            &app_schema,
                            &format!("{path}[{index}]"),
                            messages,
                            dropped,
                        );
                    }
                }
            }
        }
        toml::Value::String(_) => {
            let Some(text) = value.as_str().map(str::to_owned) else {
                *value = default.clone();
                messages.push(Message::warn(format!(
                    "{path}: invalid type; using default"
                )));
                return;
            };
            if let Some((_, _, to)) = VALUE_MIGRATIONS
                .iter()
                .find(|(name, from, _)| *name == path && *from == text)
            {
                *value = toml::Value::String((*to).to_owned());
                messages.push(Message::info(format!("{path} \"{text}\" is now \"{to}\"")));
                return;
            }
            if let Some(allowed) = choices(path) {
                if !allowed.contains(&text.as_str()) {
                    let default = default.as_str().unwrap_or_default();
                    messages.push(Message::warn(format!(
                        "{path}: invalid value {text:?}; using {default:?}"
                    )));
                    *value = toml::Value::String(default.to_owned());
                }
            }
        }
        toml::Value::Boolean(_) => {
            if !value.is_bool() {
                *value = default.clone();
                messages.push(Message::warn(format!(
                    "{path}: invalid type; using default"
                )));
            }
        }
        toml::Value::Integer(_) => {
            let Some(number) = value.as_integer() else {
                *value = default.clone();
                messages.push(Message::warn(format!(
                    "{path}: invalid type; using default"
                )));
                return;
            };
            if let Some((min, max)) = limits(path) {
                let clamped = number.clamp(min as i64, max as i64);
                if clamped != number {
                    messages.push(Message::warn(format!(
                        "{path}: {number} clamped to {clamped}"
                    )));
                }
                *value = toml::Value::Integer(clamped);
            }
        }
        toml::Value::Float(_) => {
            let number = match *value {
                toml::Value::Integer(number) => number as f64,
                toml::Value::Float(number) => number,
                _ => {
                    *value = default.clone();
                    messages.push(Message::warn(format!(
                        "{path}: invalid type; using default"
                    )));
                    return;
                }
            };
            if !number.is_finite() {
                *value = default.clone();
                messages.push(Message::warn(format!(
                    "{path}: nonfinite value; using default"
                )));
                return;
            }
            let clamped = limits(path).map_or(number, |(min, max)| number.clamp(min, max));
            if clamped != number {
                messages.push(Message::warn(format!(
                    "{path}: {number} clamped to {clamped}"
                )));
            }
            *value = toml::Value::Float(clamped);
        }
        _ => {
            *value = default.clone();
            messages.push(Message::warn(format!(
                "{path}: invalid type; using default"
            )));
        }
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
