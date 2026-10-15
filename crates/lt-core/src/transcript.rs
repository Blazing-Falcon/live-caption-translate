//! Shared UTF-8 JSONL session/terminal records for replay and the desktop app.
use crate::bus::{EventBus, EventReceiver};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::events::{FailReason, PipelineEvent};
use crate::types::{CaptureMode, CutReason, SecondPass, TextClass, Timing, UtteranceId};
use crossbeam_channel::{RecvTimeoutError, TryRecvError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSource {
    pub mode: CaptureMode,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub skip_lag_s: f64,
}

impl From<&Config> for SessionConfig {
    fn from(config: &Config) -> Self {
        Self {
            skip_lag_s: readable_f32(config.translate.skip_lag_s),
        }
    }
}

/// The translation model named in session headers.
pub fn translator_name(config: &Config) -> String {
    config.translate.model_file().0.into()
}

/// The recognizer named in session headers and `asr_live` records.
pub fn recognizer_name(config: &Config) -> String {
    format!("xasr_stream_{}ms", config.asr.chunk_ms)
}

/// The second-pass recognizer named in session headers, if it is on.
pub fn second_pass_name(config: &Config) -> Option<String> {
    (config.asr.second_pass != "off").then(|| config.asr.second_pass.clone())
}

fn readable_f32(value: f32) -> f64 {
    // f32's shortest decimal round-trips the setting without exposing binary
    // widening artifacts such as 0.4000000059604645 in the session header.
    match value.to_string().parse::<f64>() {
        Ok(decimal) => decimal,
        Err(_) => f64::from(value),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionHeader {
    pub app_version: String,
    pub started_at: String,
    pub source: SessionSource,
    pub asr: String,
    /// The second-pass recognizer; absent when it is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_second: Option<String>,
    pub translator: String,
    pub config: SessionConfig,
}

#[derive(Serialize)]
struct HeaderRecord<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    header: &'a SessionHeader,
}

struct Pending {
    id: UtteranceId,
    joined: Vec<UtteranceId>,
    start_ms: u64,
    end_ms: u64,
    class: TextClass,
    lang: Option<String>,
    source: String,
    cut: CutReason,
    live: LiveRecord,
    /// One record for each clause of the line, in order.
    second: Vec<SecondRecord>,
}

/// The second pass of one clause. `text` is the clause's final text.
#[derive(Clone, Serialize)]
struct SecondRecord {
    text: String,
    #[serde(flatten)]
    pass: SecondPass,
}

/// The live recognizer's text. A joined line concatenates its parts.
#[derive(Clone, Serialize)]
struct LiveRecord {
    engine: String,
    text: String,
    logprob: Option<f32>,
    #[serde(skip)]
    logprobs: Vec<f32>,
}

fn mean(values: &[f32]) -> Option<f32> {
    (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
}

impl LiveRecord {
    fn absorb(&mut self, other: LiveRecord) {
        self.text.push_str(&other.text);
        self.logprobs.extend(other.logprobs);
        self.logprob = mean(&self.logprobs);
    }
}

#[derive(Serialize)]
struct UtteranceRecord<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    id: UtteranceId,
    joined: &'a [UtteranceId],
    start_ms: u64,
    end_ms: u64,
    cut: CutReason,
    wall: &'a str,
    class: TextClass,
    lang: Option<&'a str>,
    source: &'a str,
    english: Option<&'a str>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    timing: Option<&'a Timing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<FailReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    asr_live: &'a LiveRecord,
    #[serde(skip_serializing_if = "<[SecondRecord]>::is_empty")]
    asr_second: &'a [SecondRecord],
}

impl Pending {
    fn record<'a>(&'a self, wall: &'a str, status: &'static str) -> UtteranceRecord<'a> {
        UtteranceRecord {
            kind: "utterance",
            id: self.id,
            joined: &self.joined,
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            cut: self.cut,
            wall,
            class: self.class,
            lang: self.lang.as_deref(),
            source: &self.source,
            english: None,
            status,
            timing: None,
            reason: None,
            message: None,
            asr_live: &self.live,
            asr_second: &self.second,
        }
    }
}

pub struct TranscriptWriter<W: Write> {
    writer: W,
    pending: BTreeMap<UtteranceId, Pending>,
    live_engine: String,
}

impl<W: Write> TranscriptWriter<W> {
    pub const PENDING_CAPACITY: usize = 1024;

    pub fn new(mut writer: W, header: SessionHeader) -> Result<Self> {
        write_line(
            &mut writer,
            &HeaderRecord {
                kind: "session",
                header: &header,
            },
        )?;
        Ok(Self {
            writer,
            pending: BTreeMap::new(),
            live_engine: header.asr.clone(),
        })
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn on_event(&mut self, event: &PipelineEvent, wall: &str) -> Result<()> {
        match event {
            PipelineEvent::AsrFinal {
                id,
                text,
                class,
                lang,
                start_ms,
                end_ms,
                cut,
                logprob,
                second_pass,
                ..
            } => {
                let pending_translation = class.is_translated();
                if pending_translation
                    && self.pending.len() == Self::PENDING_CAPACITY
                    && !self.pending.contains_key(id)
                {
                    return Err(Error::Engine("Transcript pending utterance capacity exceeded; the transcript may be incomplete".into()));
                }
                let pending = Pending {
                    id: *id,
                    joined: Vec::new(),
                    start_ms: *start_ms,
                    end_ms: *end_ms,
                    class: *class,
                    lang: lang.clone(),
                    source: text.clone(),
                    cut: *cut,
                    live: LiveRecord {
                        engine: self.live_engine.clone(),
                        text: second_pass
                            .as_ref()
                            .and_then(|pass| pass.live_text.clone())
                            .unwrap_or_else(|| text.clone()),
                        logprob: *logprob,
                        logprobs: logprob.iter().copied().collect(),
                    },
                    second: second_pass
                        .iter()
                        .map(|pass| SecondRecord {
                            text: text.clone(),
                            pass: pass.clone(),
                        })
                        .collect(),
                };
                match class {
                    TextClass::Chinese | TextClass::Mixed => {
                        self.pending.insert(*id, pending);
                    }
                    TextClass::English => {
                        write_line(&mut self.writer, &pending.record(wall, "english"))?
                    }
                    TextClass::Other => {
                        write_line(&mut self.writer, &pending.record(wall, "other"))?
                    }
                }
            }
            PipelineEvent::Joined {
                id, absorbed, text, ..
            } => {
                if let Some(mut leader) = self.pending.remove(id) {
                    leader.source = text.clone();
                    for absorbed in absorbed {
                        append_id(&mut leader.joined, leader.id, *absorbed);
                        if let Some(incoming) = self.pending.remove(absorbed) {
                            leader.end_ms = leader.end_ms.max(incoming.end_ms);
                            if incoming.class == TextClass::Mixed {
                                leader.class = TextClass::Mixed;
                            }
                            leader.live.absorb(incoming.live);
                            leader.second.extend(incoming.second);
                            for child in incoming.joined {
                                append_id(&mut leader.joined, leader.id, child);
                            }
                        }
                    }
                    self.pending.insert(*id, leader);
                }
            }
            PipelineEvent::TranslationFinal { id, text, timing } => {
                if let Some(pending) = self.pending.get(id) {
                    let mut record = pending.record(wall, "final");
                    record.english = Some(text);
                    record.timing = Some(timing);
                    write_line(&mut self.writer, &record)?;
                    self.pending.remove(id);
                }
            }
            PipelineEvent::Skipped { id, .. } => {
                if let Some(pending) = self.pending.get(id) {
                    write_line(&mut self.writer, &pending.record(wall, "skipped"))?;
                    self.pending.remove(id);
                }
            }
            PipelineEvent::TranslationFailed {
                id,
                reason,
                message,
            } => {
                if let Some(pending) = self.pending.get(id) {
                    let mut record = pending.record(wall, "failed");
                    record.reason = Some(*reason);
                    record.message = Some(message);
                    write_line(&mut self.writer, &record)?;
                    self.pending.remove(id);
                }
            }
            PipelineEvent::Dropped { id, .. } => {
                self.pending.remove(id);
            }
            _ => {}
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush().map_err(Error::Io)
    }

    /// Unfinished utterances have no terminal state and are not written.
    pub fn finish(mut self) -> Result<W> {
        self.flush()?;
        Ok(self.writer)
    }
}

/// Owns transcript I/O on a bounded event subscriber's separate thread.
///
/// Start before the pipeline publishes events. Call `finish` only after
/// `PipelineHandle::wait`/`stop` has joined every event producer, with the bus
/// still alive: this is what makes draining all queued terminal events final.
/// A receiver disconnect is an error because the bus does not distinguish
/// producer shutdown from overflow; neither is silently accepted as complete.
pub struct TranscriptSubscriber<W: Write + Send + 'static> {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<W>>>,
}

impl<W: Write + Send + 'static> TranscriptSubscriber<W> {
    pub const CAPACITY: usize = 1024;

    pub fn start(
        bus: &EventBus,
        writer: TranscriptWriter<W>,
        wall_factory: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Result<Self> {
        let receiver = bus.subscribe(Self::CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let thread = thread::Builder::new()
            .name("lt-transcript".into())
            .spawn(move || run_subscriber(receiver, writer, wall_factory, worker_stop))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    pub fn finish(mut self) -> Result<W> {
        self.stop.store(true, Ordering::Release);
        let thread = self
            .thread
            .take()
            .ok_or_else(|| Error::Engine("Transcript subscriber already finished".into()))?;
        thread
            .join()
            .map_err(|_| Error::Engine("Transcript subscriber panicked".into()))?
    }
}

impl<W: Write + Send + 'static> Drop for TranscriptSubscriber<W> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    tracing::error!(%error, "Transcript subscriber stopped with an error")
                }
                Err(_) => tracing::error!("Transcript subscriber panicked"),
            }
        }
    }
}

fn run_subscriber<W: Write + Send + 'static>(
    receiver: EventReceiver,
    mut writer: TranscriptWriter<W>,
    wall_factory: Arc<dyn Fn() -> String + Send + Sync>,
    stop: Arc<AtomicBool>,
) -> Result<W> {
    let work = (|| -> Result<()> {
        loop {
            if stop.load(Ordering::Acquire) {
                match receiver.try_recv() {
                    Ok(event) => writer.on_event(&event, &wall_factory())?,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Err(subscriber_disconnected()),
                }
            } else {
                match receiver.recv_timeout(Duration::from_millis(20)) {
                    Ok(event) => writer.on_event(&event, &wall_factory())?,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return Err(subscriber_disconnected()),
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = work {
        if let Err(flush_error) = writer.flush() {
            tracing::error!(%flush_error, "Could not flush transcript after subscriber failure");
        }
        tracing::error!(%error, "Transcript subscriber failed");
        return Err(error);
    }
    writer.finish()
}

fn subscriber_disconnected() -> Error {
    Error::Engine("Transcript event receiver disconnected; the transcript may be incomplete (subscriber overflow or event bus shutdown)".into())
}

fn append_id(joined: &mut Vec<UtteranceId>, leader: UtteranceId, id: UtteranceId) {
    if id != leader && !joined.contains(&id) {
        joined.push(id);
    }
}

fn write_line<W: Write, T: Serialize>(writer: &mut W, record: &T) -> Result<()> {
    // Serialize before writing so an invalid wire value cannot leave a partial line.
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    writer.write_all(&line).map_err(Error::Io)
}

/// Remove only generated transcript filenames older than the retention cutoff.
/// Modification time is the age marker; filename dates do not determine age.
pub fn cleanup(dir: impl AsRef<Path>, days: u32, now: SystemTime) -> Result<usize> {
    if days == 0 {
        return Ok(0);
    }
    let retention = Duration::from_secs(u64::from(days) * 86_400);
    let Some(cutoff) = now.checked_sub(retention) else {
        return Ok(0);
    };
    let entries = match fs::read_dir(dir.as_ref()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(Error::Io(error)),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_str().is_some_and(generated_filename) {
            continue;
        }
        let file_type = entry.file_type()?;
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        if entry.metadata()?.modified()? < cutoff {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn generated_filename(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() != 23
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'_'
        || &bytes[17..] != b".jsonl"
    {
        return false;
    }
    let number = |start: usize, end: usize| -> Option<u32> {
        bytes[start..end].iter().try_fold(0, |number, digit| {
            if digit.is_ascii_digit() {
                Some(number * 10 + u32::from(digit - b'0'))
            } else {
                None
            }
        })
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(13, 15),
        number(15, 17),
    ) else {
        return false;
    };
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        _ => return false,
    };
    year != 0 && day >= 1 && day <= days && hour <= 23 && minute <= 59 && second <= 59
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{DropReason, SkipReason};
    use std::fs::{File, FileTimes};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::UNIX_EPOCH;

    const WALL: &str = "2026-10-07T21:14:51+07:00";
    const GOLDEN: &str = concat!(
        "{\"type\":\"session\",\"app_version\":\"0.1.0\",\"started_at\":\"2026-10-07T21:14:03+07:00\",\"source\":{\"mode\":\"apps\",\"label\":\"Chrome\"},\"asr\":\"sensevoice-2024-07-17-int8\",\"translator\":\"hy-mt2-1.8b-q4_0\",\"config\":{\"skip_lag_s\":6.0}}\n",
        "{\"type\":\"utterance\",\"id\":20,\"joined\":[],\"start_ms\":52000,\"end_ms\":53000,\"cut\":\"pause\",\"wall\":\"2026-10-07T21:14:51+07:00\",\"class\":\"english\",\"lang\":\"en\",\"source\":\"Hello world.\",\"english\":null,\"status\":\"english\",\"asr_live\":{\"engine\":\"sensevoice-2024-07-17-int8\",\"text\":\"Hello world.\",\"logprob\":null}}\n",
        "{\"type\":\"utterance\",\"id\":21,\"joined\":[],\"start_ms\":54000,\"end_ms\":55000,\"cut\":\"pause\",\"wall\":\"2026-10-07T21:14:51+07:00\",\"class\":\"other\",\"lang\":\"ja\",\"source\":\"こんにちは。\",\"english\":null,\"status\":\"other\",\"asr_live\":{\"engine\":\"sensevoice-2024-07-17-int8\",\"text\":\"こんにちは。\",\"logprob\":null}}\n",
        "{\"type\":\"utterance\",\"id\":14,\"joined\":[],\"start_ms\":50000,\"end_ms\":51000,\"cut\":\"pause\",\"wall\":\"2026-10-07T21:14:51+07:00\",\"class\":\"chinese\",\"lang\":\"zh\",\"source\":\"来不及。\",\"english\":null,\"status\":\"skipped\",\"asr_live\":{\"engine\":\"sensevoice-2024-07-17-int8\",\"text\":\"来不及。\",\"logprob\":null}}\n",
        "{\"type\":\"utterance\",\"id\":12,\"joined\":[13],\"start_ms\":45210,\"end_ms\":47980,\"cut\":\"pause\",\"wall\":\"2026-10-07T21:14:51+07:00\",\"class\":\"chinese\",\"lang\":\"zh\",\"source\":\"我也想办一个，伟大的公司。\",\"english\":\"I also want to run a great company.\",\"status\":\"final\",\"timing\":{\"speech_end_ms\":47980,\"asr_done_ms\":48310,\"queued_ms\":48320,\"sent_ms\":48320,\"first_token_ms\":48790,\"done_ms\":49210,\"prompt_tokens\":17,\"cached_tokens\":31,\"generated_tokens\":9},\"asr_live\":{\"engine\":\"sensevoice-2024-07-17-int8\",\"text\":\"我也想办一个。伟大的公司。\",\"logprob\":null}}\n",
    );

    fn header() -> SessionHeader {
        SessionHeader {
            app_version: "0.1.0".into(),
            started_at: "2026-10-07T21:14:03+07:00".into(),
            source: SessionSource {
                mode: CaptureMode::Apps,
                label: "Chrome".into(),
            },
            asr: "sensevoice-2024-07-17-int8".into(),
            asr_second: None,
            translator: "hy-mt2-1.8b-q4_0".into(),
            config: SessionConfig { skip_lag_s: 6.0 },
        }
    }

    fn asr(
        id: u64,
        text: &str,
        class: TextClass,
        lang: &str,
        start_ms: u64,
        end_ms: u64,
    ) -> PipelineEvent {
        PipelineEvent::AsrFinal {
            id: UtteranceId(id),
            text: text.into(),
            class,
            lang: Some(lang.into()),
            start_ms,
            end_ms,
            asr_ms: 100,
            cut: crate::types::CutReason::Pause,
            logprob: None,
            second_pass: None,
        }
    }

    fn joined(id: u64, absorbed: &[u64], text: &str) -> PipelineEvent {
        PipelineEvent::Joined {
            id: UtteranceId(id),
            absorbed: absorbed.iter().copied().map(UtteranceId).collect(),
            text: text.into(),
        }
    }

    fn golden_events() -> Vec<PipelineEvent> {
        vec![
            asr(
                12,
                "我也想办一个。",
                TextClass::Chinese,
                "zh",
                45_210,
                46_300,
            ),
            asr(13, "伟大的公司。", TextClass::Chinese, "zh", 46_900, 47_980),
            joined(12, &[13], "我也想办一个，伟大的公司。"),
            asr(14, "来不及。", TextClass::Chinese, "zh", 50_000, 51_000),
            asr(20, "Hello world.", TextClass::English, "en", 52_000, 53_000),
            asr(21, "こんにちは。", TextClass::Other, "ja", 54_000, 55_000),
            asr(22, "呃。", TextClass::Chinese, "zh", 56_000, 57_000),
            PipelineEvent::Dropped {
                id: UtteranceId(22),
                reason: DropReason::Empty,
            },
            PipelineEvent::Skipped {
                id: UtteranceId(14),
                reason: SkipReason::CatchUp,
            },
            PipelineEvent::TranslationFinal {
                id: UtteranceId(12),
                text: "I also want to run a great company.".into(),
                timing: Timing {
                    speech_end_ms: 47_980,
                    asr_done_ms: 48_310,
                    queued_ms: 48_320,
                    sent_ms: 48_320,
                    first_token_ms: Some(48_790),
                    done_ms: 49_210,
                    prompt_tokens: 17,
                    cached_tokens: 31,
                    generated_tokens: 9,
                },
            },
        ]
    }

    #[test]
    fn partials_are_never_written_and_commit_clauses_record_their_cut() {
        let mut writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        let id = UtteranceId(5);
        writer
            .on_event(
                &PipelineEvent::AsrPartial {
                    id,
                    text: "这次华为".into(),
                    class: TextClass::Chinese,
                    end_ms: 1_000,
                },
                WALL,
            )
            .unwrap();
        writer
            .on_event(
                &PipelineEvent::AsrFinal {
                    id,
                    text: "这次华为发布。".into(),
                    class: TextClass::Chinese,
                    lang: Some("zh".into()),
                    start_ms: 0,
                    end_ms: 1_500,
                    asr_ms: 100,
                    cut: CutReason::Commit,
                    logprob: None,
                    second_pass: None,
                },
                WALL,
            )
            .unwrap();
        writer
            .on_event(
                &PipelineEvent::Skipped {
                    id,
                    reason: SkipReason::CatchUp,
                },
                WALL,
            )
            .unwrap();
        let output = String::from_utf8(writer.finish().unwrap()).unwrap();
        let records: Vec<serde_json::Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 2, "header and one utterance: {output}");
        assert_eq!(records[1]["cut"], "commit");
        assert!(!output.contains("Huawei") && !output.contains("这次华为\""));
    }

    #[test]
    fn terminal_event_stream_matches_the_utf8_jsonl_golden_fixture() {
        let mut writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        for event in golden_events() {
            writer.on_event(&event, WALL).unwrap();
        }
        assert_eq!(writer.pending_count(), 0);
        let output = String::from_utf8(writer.finish().unwrap()).unwrap();
        assert_eq!(output, GOLDEN);
        assert!(!output.contains("\\u6211"));
        assert_eq!(output.lines().count(), 5);
    }

    #[test]
    fn dropping_subscriber_flushes_and_joins_instead_of_abandoning_the_writer() {
        struct ObservedWriter {
            flushed: Arc<AtomicBool>,
            dropped: Arc<AtomicBool>,
        }
        impl Write for ObservedWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushed.store(true, Ordering::Release);
                Ok(())
            }
        }
        impl Drop for ObservedWriter {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }
        let bus = EventBus::default();
        let flushed = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let writer = TranscriptWriter::new(
            ObservedWriter {
                flushed: flushed.clone(),
                dropped: dropped.clone(),
            },
            header(),
        )
        .unwrap();
        let subscriber =
            TranscriptSubscriber::start(&bus, writer, Arc::new(|| WALL.to_owned())).unwrap();
        drop(subscriber);
        assert!(flushed.load(Ordering::Acquire));
        assert!(dropped.load(Ordering::Acquire));
    }

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
    struct TempDir {
        directory: PathBuf,
        workspace: PathBuf,
    }
    impl TempDir {
        fn new() -> Self {
            let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap();
            let directory = workspace.join("target/tmp").join(format!(
                "transcript-test-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).unwrap();
            assert!(directory
                .canonicalize()
                .unwrap()
                .starts_with(workspace.join("target/tmp")));
            Self {
                directory,
                workspace,
            }
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let Ok(resolved) = self.directory.canonicalize() else {
                return;
            };
            if resolved.starts_with(self.workspace.join("target/tmp")) {
                let _ = fs::remove_dir_all(resolved);
            }
        }
    }
    fn dated_file(directory: &Path, name: &str, modified: SystemTime) {
        let mut file = File::create(directory.join(name)).unwrap();
        file.write_all(b"{}\n").unwrap();
        file.set_times(FileTimes::new().set_modified(modified))
            .unwrap();
    }

    #[test]
    fn retention_removes_only_old_matching_regular_files_and_zero_keeps_forever() {
        let temp = TempDir::new();
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let old = now - Duration::from_secs(31 * 86_400);
        let cutoff = now - Duration::from_secs(30 * 86_400);
        dated_file(&temp.directory, "2026-10-07_000000.jsonl", old);
        dated_file(&temp.directory, "2026-10-07_000001.jsonl", now);
        dated_file(&temp.directory, "2026-10-07_000002.jsonl", cutoff);
        dated_file(&temp.directory, "notes.jsonl", old);
        dated_file(&temp.directory, "2026-02-29_000000.jsonl", old);
        fs::create_dir(temp.directory.join("2026-10-07_000003.jsonl")).unwrap();
        assert_eq!(cleanup(&temp.directory, 0, now).unwrap(), 0);
        assert!(temp.directory.join("2026-10-07_000000.jsonl").exists());
        assert_eq!(cleanup(&temp.directory, 30, now).unwrap(), 1);
        assert!(!temp.directory.join("2026-10-07_000000.jsonl").exists());
        for name in [
            "2026-10-07_000001.jsonl",
            "2026-10-07_000002.jsonl",
            "notes.jsonl",
            "2026-02-29_000000.jsonl",
            "2026-10-07_000003.jsonl",
        ] {
            assert!(temp.directory.join(name).exists(), "{name}");
        }
        assert_eq!(cleanup(temp.directory.join("missing"), 30, now).unwrap(), 0);
    }
}
