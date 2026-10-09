//! Selected executable sessions, per-root adapters, and a shared-clock mixer.
use super::{
    apps::{
        enumerate_capture_sessions, process_loopback_supported, selected_targets, windows_build,
        AppTarget, DiscoveryMta,
    },
    capture::{run_adapter_at, CaptureEpoch, NativeWorker, QpcClock, WorkerOptions},
    process::open_process,
};
use crate::{
    mixer::{ClockMixer, MixerPush},
    normalize::LevelNormalizer,
};
use crossbeam_channel::{bounded, Receiver};
use lt_core::{
    bus::{EventBus, EventReceiver},
    config::{AudioConfig, CaptureApp},
    error::{Error, Result},
    events::{PipelineEvent, SourceStateKind},
    source::{AudioFrame, AudioProducer, AudioSource, SourceEvents},
    types::{CaptureMode, SourceInfo, StreamTime},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const REFRESH: Duration = Duration::from_secs(1);
const MANAGER_POLL: Duration = Duration::from_millis(5);
const PREPARED_CAPACITY: usize = 64;
const MIX_WATERMARK_SAMPLES: u64 = 800; // Same 50ms delivery allowance as native gaps.

/// AudioSource for selected executable names. The session manager remains on a
/// single MTA worker; raw callbacks and prepared-frame mixing remain separate.
pub struct AppsCaptureSource {
    selected: Vec<CaptureApp>,
    audio: AudioConfig,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl AppsCaptureSource {
    /// Reads the build only. No session/device discovery or capture starts here.
    pub fn new(selected: Vec<CaptureApp>, audio: AudioConfig) -> Result<Self> {
        if !process_loopback_supported(windows_build()?) {
            return Err(Error::Engine(
                "Selected apps requires Windows build 20348 or later; choose whole system capture"
                    .into(),
            ));
        }
        Ok(Self {
            selected,
            audio,
            cancelled: Arc::new(AtomicBool::new(false)),
            worker: None,
        })
    }
}

impl AudioSource for AppsCaptureSource {
    fn start(&mut self, output: AudioProducer, events: SourceEvents) -> Result<SourceInfo> {
        if self.worker.is_some() {
            return Err(Error::Engine(
                "Selected-app capture is already running".into(),
            ));
        }
        self.cancelled = Arc::new(AtomicBool::new(false));
        let selected = self.selected.clone();
        let info = selection_info(&selected);
        let audio = self.audio.clone();
        let cancelled = self.cancelled.clone();
        self.worker = Some(
            thread::Builder::new()
                .name("lt-selected-app-manager".into())
                .spawn(move || {
                    let result = run(selected, audio, output, events.clone(), cancelled);
                    if let Err(error) = &result {
                        events.publish(PipelineEvent::SourceState {
                            state: SourceStateKind::AppsNotRunning,
                            detail: Some(error.to_string()),
                        });
                    }
                    result
                })?,
        );
        Ok(info)
    }

    fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for AppsCaptureSource {
    fn drop(&mut self) {
        self.stop();
    }
}

struct RootStream {
    frames: Receiver<AudioFrame>,
    events: EventReceiver,
    cancelled: Arc<AtomicBool>,
    adapter: Option<JoinHandle<Result<()>>>,
    pending: Option<AudioFrame>,
    ready: bool,
    created_at: Option<u64>,
}

impl RootStream {
    fn start(target: &AppTarget, audio: &AudioConfig, epoch: CaptureEpoch) -> Result<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let root_pid = target.root_pid;
        let native = NativeWorker::spawn(
            Box::new(move |_, cancelled| open_process(root_pid, cancelled)),
            WorkerOptions {
                watch_devices: false,
                unavailable_state: SourceStateKind::AppsNotRunning,
            },
            cancelled.clone(),
        )?;
        let (sender, frames) = bounded(PREPARED_CAPACITY);
        let output = AudioProducer::new(sender, cancelled.clone(), epoch.stream_time);
        let private_bus = EventBus::default();
        let events = private_bus.subscribe(32);
        let source_events = SourceEvents::new(private_bus);
        let audio = audio.clone();
        let adapter = thread::Builder::new()
            .name(format!("lt-app-adapter-{root_pid}"))
            .spawn(move || run_adapter_at(native, output, source_events, audio, epoch))?;
        Ok(Self {
            frames,
            events,
            cancelled,
            adapter: Some(adapter),
            pending: None,
            ready: false,
            created_at: target.created_at,
        })
    }

    fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(adapter) = self.adapter.take() {
            let _ = adapter.join();
        }
    }
}
impl Drop for RootStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn selection_info(selected: &[CaptureApp]) -> SourceInfo {
    let names: BTreeSet<_> = selected
        .iter()
        .filter(|app| !app.exe.is_empty())
        .map(|app| {
            if app.name.is_empty() {
                app.exe.as_str()
            } else {
                app.name.as_str()
            }
        })
        .collect();
    SourceInfo {
        mode: CaptureMode::Apps,
        label: if names.is_empty() {
            "Selected apps".into()
        } else {
            names.into_iter().collect::<Vec<_>>().join(", ")
        },
        sample_rate: 48_000,
        channels: 2,
    }
}

fn source_state(
    events: &SourceEvents,
    last: &mut Option<SourceStateKind>,
    state: SourceStateKind,
    detail: Option<String>,
) {
    if *last != Some(state) {
        events.publish(PipelineEvent::SourceState { state, detail });
        *last = Some(state);
    }
}

fn run(
    selected: Vec<CaptureApp>,
    audio: AudioConfig,
    output: AudioProducer,
    events: SourceEvents,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let _com = DiscoveryMta::new()?;
    let clock = QpcClock::new()?;
    let epoch = CaptureEpoch {
        qpc_100ns: clock.now()?,
        stream_time: output.base_time,
    };
    run_manager(
        selected,
        audio,
        output,
        events,
        cancelled.clone(),
        ManagerContext {
            epoch,
            clock,
            refresh: REFRESH,
        },
        NativeBackend { cancelled },
    )
}

trait ManagerBackend {
    fn targets(&mut self, selected: &[CaptureApp]) -> Result<Vec<AppTarget>>;
    fn open(
        &mut self,
        target: &AppTarget,
        audio: &AudioConfig,
        epoch: CaptureEpoch,
    ) -> Result<RootStream>;
}

struct NativeBackend {
    cancelled: Arc<AtomicBool>,
}
impl ManagerBackend for NativeBackend {
    fn targets(&mut self, selected: &[CaptureApp]) -> Result<Vec<AppTarget>> {
        enumerate_capture_sessions(&self.cancelled)
            .map(|snapshot| selected_targets(selected, &snapshot))
    }
    fn open(
        &mut self,
        target: &AppTarget,
        audio: &AudioConfig,
        epoch: CaptureEpoch,
    ) -> Result<RootStream> {
        RootStream::start(target, audio, epoch)
    }
}

struct ManagerContext {
    epoch: CaptureEpoch,
    clock: QpcClock,
    refresh: Duration,
}

fn run_manager(
    selected: Vec<CaptureApp>,
    audio: AudioConfig,
    mut output: AudioProducer,
    events: SourceEvents,
    cancelled: Arc<AtomicBool>,
    context: ManagerContext,
    mut backend: impl ManagerBackend,
) -> Result<()> {
    let ManagerContext {
        epoch,
        clock,
        refresh,
    } = context;
    let pipeline_cancelled = output.cancelled.clone();
    // Source.stop() can run while the core channel is full during mode changes.
    // Use our local token for send so that joining never depends on global stop.
    output.cancelled = cancelled.clone();
    let mut mixer = ClockMixer::new(epoch.stream_time);
    let mut normalizer = LevelNormalizer::new(audio.clone());
    let root_audio = AudioConfig {
        normalize: false,
        ..audio
    };
    let mut roots = BTreeMap::<u32, RootStream>::new();
    let mut next_refresh = Instant::now();
    let mut last_state = None;
    let mut last_real = epoch.stream_time;
    let info = selection_info(&selected);
    let selected_empty = selected.iter().all(|app| app.exe.is_empty());
    while !cancelled.load(Ordering::Acquire) && !pipeline_cancelled.load(Ordering::Acquire) {
        if Instant::now() >= next_refresh {
            next_refresh = Instant::now() + refresh;
            let targets = if selected_empty {
                Some(Vec::new())
            } else {
                match backend.targets(&selected) {
                    Ok(targets) => Some(targets),
                    Err(Error::Stopped) => return Ok(()),
                    Err(error) => {
                        tracing::warn!(%error, "Audio-session refresh failed; retaining current sources");
                        if roots.is_empty() {
                            source_state(
                                &events,
                                &mut last_state,
                                SourceStateKind::AppsNotRunning,
                                Some(error.to_string()),
                            );
                        }
                        None
                    }
                }
            };
            if let Some(targets) = targets {
                let wanted: BTreeSet<_> = targets.iter().map(|target| target.root_pid).collect();
                let gone: Vec<_> = roots
                    .keys()
                    .copied()
                    .filter(|pid| !wanted.contains(pid))
                    .collect();
                for pid in gone {
                    roots.remove(&pid);
                    mixer.remove_source(u64::from(pid));
                    events.publish(PipelineEvent::SourceChanged { info: info.clone() });
                }
                for target in targets {
                    let restarted = roots.get(&target.root_pid).is_some_and(|root| {
                    matches!((root.created_at, target.created_at), (Some(previous), Some(current)) if previous != current)
                        || root.adapter.as_ref().is_some_and(JoinHandle::is_finished)
                });
                    if restarted {
                        roots.remove(&target.root_pid);
                        mixer.reset_source(u64::from(target.root_pid));
                    }
                    if let Some(root) = roots.get_mut(&target.root_pid) {
                        root.created_at = root.created_at.or(target.created_at);
                    }
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        roots.entry(target.root_pid)
                    {
                        let registration = if restarted {
                            Ok(())
                        } else {
                            mixer.add_source(u64::from(target.root_pid))
                        };
                        if let Err(error) = registration {
                            tracing::warn!(%error, pid = target.root_pid, "Cannot add selected-app capture source");
                            continue;
                        }
                        let source = match backend.open(&target, &root_audio, epoch) {
                            Ok(source) => source,
                            Err(Error::Stopped) => return Ok(()),
                            Err(error) => {
                                mixer.remove_source(u64::from(target.root_pid));
                                tracing::warn!(%error, pid = target.root_pid, "Cannot open selected-app capture source; retrying on refresh");
                                continue;
                            }
                        };
                        entry.insert(source);
                        events.publish(PipelineEvent::SourceChanged { info: info.clone() });
                    }
                }
            }
        }
        for (pid, root) in &mut roots {
            for event in root.events.try_iter() {
                match event {
                    PipelineEvent::SourceChanged { .. } => {
                        // Metadata and prepared audio use different queues. Do
                        // not discard fresh audio which may already follow this
                        // notice: the adapter marks the first new frame with
                        // DISCONTINUITY, consumed in timestamp order by the mixer.
                        root.ready = true;
                        events.publish(PipelineEvent::SourceChanged { info: info.clone() });
                    }
                    PipelineEvent::SourceState {
                        state: SourceStateKind::AppsNotRunning,
                        ..
                    } => root.ready = false,
                    _ => {}
                }
            }
            while let Some(frame) = root.pending.take().or_else(|| root.frames.try_recv().ok()) {
                match mixer.push(u64::from(*pid), frame)? {
                    MixerPush::Accepted | MixerPush::Late => {}
                    MixerPush::Full(frame) => {
                        root.pending = Some(frame);
                        break;
                    }
                }
            }
        }
        let now = qpc_stream_time(epoch, clock.now()?);
        let available = StreamTime(
            now.samples()
                .saturating_sub(MIX_WATERMARK_SAMPLES)
                .max(epoch.stream_time.samples()),
        );
        while let Some(mut frame) = mixer.take_until(available) {
            let has_audio = frame.samples.iter().any(|sample| *sample != 0.0);
            if has_audio {
                last_real = StreamTime(frame.t0.samples().saturating_add(512));
            }
            if selected_empty {
                source_state(
                    &events,
                    &mut last_state,
                    SourceStateKind::NoAppsSelected,
                    None,
                );
            } else if roots.is_empty() || roots.values().all(|root| !root.ready) {
                source_state(
                    &events,
                    &mut last_state,
                    SourceStateKind::AppsNotRunning,
                    None,
                );
            } else if has_audio {
                source_state(&events, &mut last_state, SourceStateKind::Playing, None);
            } else if now.saturating_sub(last_real).samples() >= 3 * 16_000 {
                source_state(&events, &mut last_state, SourceStateKind::Silent, None);
            }
            normalize_mixed_frame(&mut frame, &mut normalizer);
            match output.send(frame) {
                Ok(()) => {}
                Err(Error::Stopped) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        thread::sleep(MANAGER_POLL);
    }
    // RootStream Drop sets every source token and joins its adapter/native worker.
    roots.clear();
    Ok(())
}

fn qpc_stream_time(epoch: CaptureEpoch, qpc: u64) -> StreamTime {
    let elapsed = qpc.saturating_sub(epoch.qpc_100ns);
    let samples = (u128::from(elapsed) * 16_000 / 10_000_000).min(u128::from(u64::MAX)) as u64;
    StreamTime(epoch.stream_time.samples().saturating_add(samples))
}

fn normalize_mixed_frame(frame: &mut AudioFrame, normalizer: &mut LevelNormalizer) {
    if frame.flags.discontinuity {
        normalizer.reset();
    }
    normalizer.process(&mut frame.samples);
    // AGC release/attack can retain quiet-speech gain when a loud app starts.
    // Bound the final PCM after that gain, outside every native callback.
    for sample in &mut frame.samples {
        *sample = sample.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeBackend {
        calls: usize,
        opened: Arc<std::sync::atomic::AtomicU32>,
        closed: Arc<std::sync::atomic::AtomicU32>,
    }

    impl ManagerBackend for FakeBackend {
        fn targets(&mut self, _: &[CaptureApp]) -> Result<Vec<AppTarget>> {
            self.calls += 1;
            let pids: &[u32] = match self.calls {
                1 => &[10],
                2 => &[10, 20],
                3 => &[20],
                _ => &[40, 20],
            };
            Ok(pids
                .iter()
                .map(|pid| AppTarget {
                    root_pid: *pid,
                    exe: if *pid == 20 {
                        "discord.exe"
                    } else {
                        "chrome.exe"
                    }
                    .into(),
                    name: if *pid == 20 { "Discord" } else { "Chrome" }.into(),
                    created_at: Some(u64::from(*pid)),
                })
                .collect())
        }

        fn open(
            &mut self,
            target: &AppTarget,
            audio: &AudioConfig,
            epoch: CaptureEpoch,
        ) -> Result<RootStream> {
            assert!(!audio.normalize, "Only the aggregate is normalized");
            self.opened.fetch_add(1, Ordering::Relaxed);
            let closed = self.closed.clone();
            let amplitude = if target.root_pid == 20 { 0.2 } else { 0.1 };
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = cancelled.clone();
            let (sender, frames) = bounded(PREPARED_CAPACITY);
            let bus = EventBus::default();
            let events = bus.subscribe(8);
            let source_events = SourceEvents::new(bus);
            let created_at = target.created_at;
            let adapter = thread::spawn(move || {
                source_events.publish(PipelineEvent::SourceChanged {
                    info: selection_info(&[]),
                });
                let producer = AudioProducer::new(sender, flag.clone(), epoch.stream_time);
                let clock = QpcClock::new()?;
                let mut t0 = qpc_stream_time(epoch, clock.now()?);
                while !flag.load(Ordering::Acquire) {
                    if qpc_stream_time(epoch, clock.now()?).samples()
                        >= t0.samples().saturating_add(512)
                    {
                        let frame = AudioFrame {
                            t0,
                            samples: [amplitude; 512],
                            flags: lt_core::segment::FrameFlags::EMPTY,
                        };
                        if producer.send(frame).is_err() {
                            break;
                        }
                        t0 = StreamTime(t0.samples().saturating_add(512));
                    } else {
                        thread::sleep(Duration::from_millis(2));
                    }
                }
                closed.fetch_add(1, Ordering::Relaxed);
                Ok(())
            });
            Ok(RootStream {
                frames,
                events,
                cancelled,
                adapter: Some(adapter),
                pending: None,
                ready: false,
                created_at,
            })
        }
    }

    #[test]
    fn source_stop_unblocks_a_full_output_even_when_pipeline_keeps_running() {
        let clock = QpcClock::new().unwrap();
        let epoch = CaptureEpoch {
            qpc_100ns: clock.now().unwrap(),
            stream_time: StreamTime::ZERO,
        };
        let context = ManagerContext {
            clock,
            epoch,
            refresh: REFRESH,
        };
        let closed = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let backend = FakeBackend {
            calls: 0,
            opened: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            closed: closed.clone(),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let pipeline_flag = Arc::new(AtomicBool::new(false));
        let (sender, _unread) = bounded(1);
        let output = AudioProducer::new(sender, pipeline_flag.clone(), StreamTime::ZERO);
        let source_events = SourceEvents::new(EventBus::default());
        let selected = vec![CaptureApp {
            exe: "chrome.exe".into(),
            ..CaptureApp::default()
        }];
        let worker = thread::spawn(move || {
            run_manager(
                selected,
                AudioConfig::default(),
                output,
                source_events,
                flag,
                context,
                backend,
            )
        });
        thread::sleep(Duration::from_millis(160));
        let started = Instant::now();
        cancelled.store(true, Ordering::Release);
        worker.join().unwrap().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!pipeline_flag.load(Ordering::Acquire));
        assert_eq!(closed.load(Ordering::Relaxed), 1);
    }
}
