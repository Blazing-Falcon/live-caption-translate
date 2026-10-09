//! Shared UTF-8 JSONL session/terminal records for replay and the desktop app.
use crate::bus::{EventBus, EventReceiver};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::events::{FailReason, PipelineEvent};
use crate::metrics::{canonical_language, OtherRouting};
use crate::types::{CaptureMode, CutReason, TextClass, Timing, UtteranceId};
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
    pub min_silence_s: f64,
    pub hold_max_chars: u32,
    pub skip_lag_s: f64,
    // Later fields (older transcripts lack these)
    /// The caption speed setting at the start of the session.
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub comma_min_tokens: u32,
    #[serde(default)]
    pub cap_tokens: u32,
    #[serde(default)]
    pub decode_interval_s: f64,
    /// The draft model in use, or `null` when the session runs without drafts.
    #[serde(default)]
    pub draft_model: Option<String>,
}

/// Manifest id of the draft model, recorded in the session header when drafts run.
pub const DRAFT_MODEL_ID: &str = "lmt-60-0.6b-q4_k_m";

impl SessionConfig {
    /// Record whether a draft translator is running in this session.
    pub fn with_draft(mut self, running: bool) -> Self {
        self.draft_model = running.then(|| DRAFT_MODEL_ID.to_owned());
        self
    }
}

impl From<&Config> for SessionConfig {
    fn from(config: &Config) -> Self {
        Self {
            min_silence_s: readable_f32(config.vad.min_silence_s),
            hold_max_chars: config.join.hold_max_chars,
            skip_lag_s: readable_f32(config.translate.skip_lag_s),
            mode: config.latency.mode.clone(),
            comma_min_tokens: config.latency.comma_min_tokens,
            cap_tokens: if config.latency.split_long {
                config.latency.cap_tokens
            } else {
                0
            },
            decode_interval_s: readable_f32(config.latency.decode_interval_s),
            draft_model: None,
        }
    }
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
        }
    }
}

pub struct TranscriptWriter<W: Write> {
    writer: W,
    pending: BTreeMap<UtteranceId, Pending>,
    routing: OtherRouting,
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
            routing: OtherRouting::default(),
        })
    }

    pub fn with_routing(
        writer: W,
        header: SessionHeader,
        translate_other: &[String],
    ) -> Result<Self> {
        let mut writer = Self::new(writer, header)?;
        writer.set_translate_other(translate_other);
        Ok(writer)
    }

    /// Configure before consuming events; a routing change starts a new pipeline.
    pub fn set_translate_other(&mut self, translate_other: &[String]) {
        self.routing.set(translate_other);
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
                ..
            } => {
                let pending_translation = matches!(class, TextClass::Chinese | TextClass::Mixed)
                    || (*class == TextClass::Other && self.routing.translates(lang.as_deref()));
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
                    lang: lang.as_deref().map(canonical_language),
                    source: text.clone(),
                    cut: *cut,
                };
                match class {
                    TextClass::Chinese | TextClass::Mixed => {
                        self.pending.insert(*id, pending);
                    }
                    TextClass::English => {
                        write_line(&mut self.writer, &pending.record(wall, "english"))?
                    }
                    TextClass::Other if self.routing.translates(lang.as_deref()) => {
                        self.pending.insert(*id, pending);
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
    use crate::events::{DropReason, JoinKind, SkipReason};
    use std::fs::{File, FileTimes};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::UNIX_EPOCH;

    const WALL: &str = "2026-10-07T21:14:51+07:00";
    const GOLDEN: &str = concat!(
        "{\"type\":\"session\",\"app_version\":\"0.1.0\",\"started_at\":\"2026-10-07T21:14:03+07:00\",\"source\":{\"mode\":\"apps\",\"label\":\"Chrome\"},\"asr\":\"sensevoice-2024-07-17-int8\",\"translator\":\"hy-mt2-1.8b-q4_0\",\"config\":{\"min_silence_s\":0.4,\"hold_max_chars\":8,\"skip_lag_s\":6.0,\"mode\":\"auto\",\"comma_min_tokens\":8,\"cap_tokens\":20,\"decode_interval_s\":0.5,\"draft_model\":null}}\n",
        "{\"type\":\"utterance\",\"id\":20,\"joined\":[],\"start_ms\":52000,\"end_ms\":53000,\"cut\":\"pause\",\"wall\":\"2026-10-07T21:14:51+07:00\",\"class\":\"english\",\"lang\":\"en\",\"source\":\"Hello world.\",\"english\":null,\"status\":\"english\"}\n",
        "{\"type\":\"utterance\",\"id\":21,\"joined\":[],\"start_ms\":54000,\"end_ms\":55000,\"cut\":\"pause\",\"wall\":\"2026-10-07T21:14:51+07:00\",\"class\":\"other\",\"lang\":\"ja\",\"source\":\"こんにちは。\",\"english\":null,\"status\":\"other\"}\n",
        "{\"type\":\"utterance\",\"id\":14,\"joined\":[],\"start_ms\":50000,\"end_ms\":51000,\"cut\":\"pause\",\"wall\":\"2026-10-07T21:14:51+07:00\",\"class\":\"chinese\",\"lang\":\"zh\",\"source\":\"来不及。\",\"english\":null,\"status\":\"skipped\"}\n",
        "{\"type\":\"utterance\",\"id\":12,\"joined\":[13],\"start_ms\":45210,\"end_ms\":47980,\"cut\":\"pause\",\"wall\":\"2026-10-07T21:14:51+07:00\",\"class\":\"chinese\",\"lang\":\"zh\",\"source\":\"我也想办一个，伟大的公司。\",\"english\":\"I also want to run a great company.\",\"status\":\"final\",\"timing\":{\"speech_end_ms\":47980,\"asr_done_ms\":48310,\"queued_ms\":48320,\"sent_ms\":48320,\"first_token_ms\":48790,\"done_ms\":49210,\"prompt_tokens\":17,\"cached_tokens\":31,\"generated_tokens\":9}}\n",
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
            translator: "hy-mt2-1.8b-q4_0".into(),
            config: SessionConfig {
                min_silence_s: 0.4,
                hold_max_chars: 8,
                skip_lag_s: 6.0,
                mode: "auto".into(),
                comma_min_tokens: 8,
                cap_tokens: 20,
                decode_interval_s: 0.5,
                draft_model: None,
            },
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
        }
    }

    fn joined(id: u64, absorbed: &[u64], text: &str, kind: JoinKind) -> PipelineEvent {
        PipelineEvent::Joined {
            id: UtteranceId(id),
            absorbed: absorbed.iter().copied().map(UtteranceId).collect(),
            text: text.into(),
            kind,
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
            joined(12, &[13], "我也想办一个，伟大的公司。", JoinKind::Hold),
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
    fn partials_and_drafts_are_never_written_and_commit_clauses_record_their_cut() {
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
                &PipelineEvent::TranslationDraft {
                    id,
                    rev: 1,
                    text: "This time Huawei".into(),
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
    fn subscriber_drains_queued_terminal_events_to_the_exact_golden_before_joining() {
        let bus = EventBus::default();
        let writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        let subscriber =
            TranscriptSubscriber::start(&bus, writer, Arc::new(|| WALL.to_owned())).unwrap();
        for event in golden_events() {
            bus.publish(event);
        }
        // All producers are done; finishing must drain events even if the worker
        // has not yet read the final that was just published.
        let output = String::from_utf8(subscriber.finish().unwrap()).unwrap();
        assert_eq!(output, GOLDEN);
    }

    #[test]
    fn writer_pending_capacity_fails_explicitly_and_existing_ids_can_still_update() {
        let mut writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        for id in 1..=TranscriptWriter::<Vec<u8>>::PENDING_CAPACITY as u64 {
            writer
                .on_event(&asr(id, "我们。", TextClass::Chinese, "zh", 100, 200), WALL)
                .unwrap();
        }
        writer
            .on_event(&asr(1, "改过。", TextClass::Chinese, "zh", 100, 200), WALL)
            .unwrap();
        let error = writer
            .on_event(
                &asr(2_000, "新的。", TextClass::Chinese, "zh", 300, 400),
                WALL,
            )
            .unwrap_err();
        assert!(error.to_string().contains("capacity exceeded"));
        assert_eq!(
            writer.pending_count(),
            TranscriptWriter::<Vec<u8>>::PENDING_CAPACITY
        );
    }

    #[test]
    fn subscriber_overflow_returns_incomplete_stream_error_even_during_finish() {
        let bus = EventBus::default();
        let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded(1);
        let first = AtomicBool::new(true);
        let wall = Arc::new(move || {
            if first.swap(false, Ordering::AcqRel) {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            WALL.to_owned()
        });
        let writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        let subscriber = TranscriptSubscriber::start(&bus, writer, wall).unwrap();
        bus.publish(PipelineEvent::SpeechStarted {
            id: UtteranceId(1),
            at_ms: 0,
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        for id in 2..=TranscriptSubscriber::<Vec<u8>>::CAPACITY as u64 + 2 {
            bus.publish(PipelineEvent::SpeechStarted {
                id: UtteranceId(id),
                at_ms: 0,
            });
        }
        release_tx.send(()).unwrap();
        let error = subscriber.finish().unwrap_err().to_string();
        assert!(error.contains("disconnected"));
        assert!(error.contains("incomplete"));
    }

    #[test]
    fn subscriber_reports_terminal_write_failure_and_joins_its_thread() {
        struct FailAfterHeader {
            header_written: bool,
        }
        impl Write for FailAfterHeader {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.header_written {
                    return Err(std::io::Error::other("terminal write failed"));
                }
                self.header_written = true;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let bus = EventBus::default();
        let writer = TranscriptWriter::new(
            FailAfterHeader {
                header_written: false,
            },
            header(),
        )
        .unwrap();
        let subscriber =
            TranscriptSubscriber::start(&bus, writer, Arc::new(|| WALL.to_owned())).unwrap();
        bus.publish(asr(1, "Hello world.", TextClass::English, "en", 100, 200));
        let error = match subscriber.finish() {
            Ok(_) => panic!("the terminal write must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, Error::Io(_)));
        assert_eq!(error.to_string(), "terminal write failed");
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

    #[test]
    fn bus_shutdown_is_reported_when_the_caller_did_not_keep_it_alive() {
        let bus = EventBus::default();
        let writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        let subscriber =
            TranscriptSubscriber::start(&bus, writer, Arc::new(|| WALL.to_owned())).unwrap();
        drop(bus);
        assert!(subscriber
            .finish()
            .unwrap_err()
            .to_string()
            .contains("event bus shutdown"));
    }

    #[test]
    fn joins_of_joined_groups_preserve_all_ids_latest_end_and_mixed_class() {
        let mut writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        for event in [
            asr(1, "我们。", TextClass::Chinese, "zh", 100, 200),
            asr(2, "今天。", TextClass::Chinese, "zh", 300, 400),
            joined(1, &[2], "我们，今天。", JoinKind::Hold),
            asr(3, "谈 privacy。", TextClass::Mixed, "zh", 500, 600),
            asr(4, "话题。", TextClass::Chinese, "zh", 700, 800),
            joined(3, &[4], "谈 privacy，话题。", JoinKind::Hold),
            joined(1, &[3], "我们，今天，谈 privacy，话题。", JoinKind::Queue),
            PipelineEvent::Skipped {
                id: UtteranceId(1),
                reason: SkipReason::CatchUp,
            },
            PipelineEvent::TranslationFinal {
                id: UtteranceId(2),
                text: "stale absorbed event".into(),
                timing: Timing::default(),
            },
        ] {
            writer.on_event(&event, WALL).unwrap();
        }
        let output = String::from_utf8(writer.finish().unwrap()).unwrap();
        assert_eq!(output.lines().count(), 2);
        let record: serde_json::Value =
            serde_json::from_str(output.lines().nth(1).unwrap()).unwrap();
        assert_eq!(record["joined"], serde_json::json!([2, 3, 4]));
        assert_eq!(record["class"], "mixed");
        assert_eq!(record["start_ms"], 100);
        assert_eq!(record["end_ms"], 800);
        assert!(record["english"].is_null());
    }

    #[test]
    fn failures_log_reason_and_message_and_unfinished_lines_are_not_written() {
        let mut writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        writer
            .on_event(&asr(1, "我们。", TextClass::Chinese, "zh", 100, 200), WALL)
            .unwrap();
        writer
            .on_event(
                &asr(2, "还在等。", TextClass::Chinese, "zh", 300, 400),
                WALL,
            )
            .unwrap();
        writer
            .on_event(
                &PipelineEvent::TranslationFailed {
                    id: UtteranceId(1),
                    reason: FailReason::Timeout,
                    message: "Translation took too long".into(),
                },
                WALL,
            )
            .unwrap();
        writer.flush().unwrap();
        let output = String::from_utf8(writer.finish().unwrap()).unwrap();
        assert_eq!(output.lines().count(), 2);
        let record: serde_json::Value =
            serde_json::from_str(output.lines().nth(1).unwrap()).unwrap();
        assert_eq!(record["status"], "failed");
        assert_eq!(record["reason"], "timeout");
        assert_eq!(record["message"], "Translation took too long");
        assert!(record["english"].is_null());
        assert!(record.get("timing").is_none());
    }

    #[test]
    fn enabled_japanese_and_korean_log_the_translated_final_in_terminal_order() {
        let mut writer = TranscriptWriter::with_routing(
            Vec::new(),
            header(),
            &["Japanese".into(), "ko_KR".into()],
        )
        .unwrap();
        writer
            .on_event(
                &asr(1, "こんにちは。", TextClass::Other, "<|JA|>", 100, 200),
                WALL,
            )
            .unwrap();
        writer
            .on_event(
                &asr(2, "안녕하세요.", TextClass::Other, "KO-kr", 300, 400),
                WALL,
            )
            .unwrap();
        assert_eq!(writer.pending_count(), 2);
        assert_eq!(
            writer.writer.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
        for (id, text) in [(2, "Hello."), (1, "Good afternoon.")] {
            writer
                .on_event(
                    &PipelineEvent::TranslationFinal {
                        id: UtteranceId(id),
                        text: text.into(),
                        timing: Timing::default(),
                    },
                    WALL,
                )
                .unwrap();
        }
        assert_eq!(writer.pending_count(), 0);
        let output = String::from_utf8(writer.finish().unwrap()).unwrap();
        let records: Vec<serde_json::Value> = output
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["id"], 2);
        assert_eq!(records[0]["lang"], "ko");
        assert_eq!(records[0]["english"], "Hello.");
        assert_eq!(records[1]["id"], 1);
        assert_eq!(records[1]["lang"], "ja");
        assert_eq!(records[1]["english"], "Good afternoon.");
        for record in records {
            assert_eq!(record["class"], "other");
            assert_eq!(record["status"], "final");
            assert!(record.get("timing").is_some());
        }
    }

    #[test]
    fn enabled_other_skip_and_failure_wait_for_terminal_but_disabled_other_is_immediate() {
        let mut writer = TranscriptWriter::new(Vec::new(), header()).unwrap();
        writer.set_translate_other(&["ja".into()]);
        writer
            .on_event(
                &asr(1, "こんにちは。", TextClass::Other, "ja", 100, 200),
                WALL,
            )
            .unwrap();
        writer
            .on_event(
                &asr(2, "おはよう。", TextClass::Other, "ja", 300, 400),
                WALL,
            )
            .unwrap();
        writer
            .on_event(
                &asr(3, "안녕하세요.", TextClass::Other, "ko", 500, 600),
                WALL,
            )
            .unwrap();
        assert_eq!(writer.pending_count(), 2);
        assert_eq!(
            writer.writer.iter().filter(|byte| **byte == b'\n').count(),
            2
        );
        writer
            .on_event(
                &PipelineEvent::Skipped {
                    id: UtteranceId(1),
                    reason: SkipReason::CatchUp,
                },
                WALL,
            )
            .unwrap();
        writer
            .on_event(
                &PipelineEvent::TranslationFailed {
                    id: UtteranceId(2),
                    reason: FailReason::Timeout,
                    message: "Timed out".into(),
                },
                WALL,
            )
            .unwrap();
        assert_eq!(writer.pending_count(), 0);
        let output = String::from_utf8(writer.finish().unwrap()).unwrap();
        let records: Vec<serde_json::Value> = output
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records[0]["id"], 3);
        assert_eq!(records[0]["status"], "other");
        assert_eq!(records[1]["status"], "skipped");
        assert_eq!(records[2]["status"], "failed");
        assert!(records.iter().all(|record| record["english"].is_null()));
    }

    #[test]
    fn session_config_keeps_readable_setting_decimals() {
        let config = Config::default();
        let subset = SessionConfig::from(&config);
        assert_eq!(subset.min_silence_s, 0.4);
        assert_eq!(subset.skip_lag_s, 6.0);
        assert_eq!(
            serde_json::to_string(&subset).unwrap(),
            "{\"min_silence_s\":0.4,\"hold_max_chars\":8,\"skip_lag_s\":6.0,\"mode\":\"auto\",\"comma_min_tokens\":8,\"cap_tokens\":20,\"decode_interval_s\":0.5,\"draft_model\":null}"
        );
    }

    #[test]
    fn io_errors_propagate_from_header_and_flush() {
        struct FailWrite;
        impl Write for FailWrite {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("write failed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(TranscriptWriter::new(FailWrite, header()).is_err());
        struct FailFlush(Vec<u8>);
        impl Write for FailFlush {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.write(bytes)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("flush failed"))
            }
        }
        let mut writer = TranscriptWriter::new(FailFlush(Vec::new()), header()).unwrap();
        assert!(writer.flush().is_err());
        assert!(writer.finish().is_err());
    }

    #[test]
    fn filenames_require_actual_calendar_dates_and_exact_generated_shape() {
        for name in [
            "2026-10-07_211403.jsonl",
            "2024-02-29_000000.jsonl",
            "2000-02-29_235959.jsonl",
        ] {
            assert!(generated_filename(name), "{name}");
        }
        for name in [
            "notes.jsonl",
            "2026-10-07_211403.JSONL",
            "2026-10-07_211403.jsonl.bak",
            "2026-02-29_000000.jsonl",
            "1900-02-29_000000.jsonl",
            "2026-04-31_000000.jsonl",
            "0000-10-07_000000.jsonl",
            "2026-00-07_000000.jsonl",
            "2026-10-00_000000.jsonl",
            "2026-10-07_240000.jsonl",
            "2026-10-07_006000.jsonl",
            "2026-10-07_000060.jsonl",
            "2026-10-07_00a000.jsonl",
            "四四四四-10-07_000000.jsonl",
        ] {
            assert!(!generated_filename(name), "{name}");
        }
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
