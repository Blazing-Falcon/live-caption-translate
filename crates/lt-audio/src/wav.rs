//! Headless WAV replay with bounded decoding buffers and a prepared 16 kHz seam.

use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use hound::{SampleFormat, WavReader, WavSpec};
use lt_core::{
    config::AudioConfig,
    error::{Error, Result},
    events::{PipelineEvent, SourceStateKind},
    segment::FrameFlags,
    source::{AudioFrame, AudioProducer, AudioSource, SourceEvents},
    types::{CaptureMode, SourceInfo, StreamTime, FRAME_SAMPLES, SAMPLE_RATE},
};

use crate::{downmix::downmix_interleaved, normalize::LevelNormalizer, resample::MonoResampler};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pace {
    RealTime,
    AsFastAsPossible,
}

/// A missing-packet interval relative to the selected replay excerpt. Its audio
/// is replaced by zeros, preserving the source timeline and setting GAP_FILLED.
#[derive(Clone, Copy, Debug)]
pub struct InjectedGap {
    pub start: StreamTime,
    pub duration: StreamTime,
}

pub struct WavSource {
    path: PathBuf,
    spec: WavSpec,
    file_frames: u32,
    start_frame: u32,
    end_frame: u32,
    pace: Pace,
    audio: AudioConfig,
    gaps: Vec<InjectedGap>,
    position: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    cancelled: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl WavSource {
    pub fn new(path: impl AsRef<Path>, pace: Pace) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let reader = WavReader::open(&path).map_err(wav_error)?;
        let spec = reader.spec();
        if spec.channels == 0 || spec.sample_rate == 0 || !(1..=32).contains(&spec.bits_per_sample)
        {
            return Err(Error::Engine(
                "WAV has an unsupported channel/rate/sample format".into(),
            ));
        }
        if spec.sample_format == SampleFormat::Float && spec.bits_per_sample != 32 {
            return Err(Error::Engine(
                "WAV floating-point samples must be 32-bit".into(),
            ));
        }
        let file_frames = reader.duration();
        Ok(Self {
            path,
            spec,
            file_frames,
            start_frame: 0,
            end_frame: file_frames,
            pace,
            audio: AudioConfig::default(),
            gaps: Vec::new(),
            position: Arc::new(AtomicU64::new(0)),
            error: Arc::new(Mutex::new(None)),
            cancelled: None,
            worker: None,
        })
    }

    pub fn with_audio_config(mut self, audio: AudioConfig) -> Self {
        self.audio = audio;
        self
    }

    pub fn with_range(mut self, start_s: f64, duration_s: Option<f64>) -> Result<Self> {
        if !start_s.is_finite()
            || start_s < 0.0
            || duration_s.is_some_and(|duration| !duration.is_finite() || duration < 0.0)
        {
            return Err(Error::Engine(
                "Replay start and duration must be finite nonnegative seconds".into(),
            ));
        }
        self.start_frame = ((start_s * f64::from(self.spec.sample_rate)).floor() as u64)
            .min(u64::from(self.file_frames)) as u32;
        self.end_frame = duration_s.map_or(self.file_frames, |duration| {
            u64::from(self.start_frame)
                .saturating_add((duration * f64::from(self.spec.sample_rate)).floor() as u64)
                .min(u64::from(self.file_frames)) as u32
        });
        self.position.store(0, Ordering::Release);
        Ok(self)
    }

    pub fn with_gaps(mut self, gaps: Vec<InjectedGap>) -> Result<Self> {
        if gaps.iter().any(|gap| gap.duration.samples() == 0) {
            return Err(Error::Engine(
                "Injected gaps must have a positive duration".into(),
            ));
        }
        self.gaps = gaps;
        Ok(self)
    }

    /// Keep this observer before boxing the source to inspect decode errors after replay.
    pub fn error_handle(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.error)
    }

    pub fn source_info(&self) -> SourceInfo {
        SourceInfo {
            mode: CaptureMode::System,
            label: self.path.file_name().map_or_else(
                || "WAV replay".into(),
                |name| name.to_string_lossy().into_owned(),
            ),
            sample_rate: self.spec.sample_rate,
            channels: self.spec.channels,
        }
    }
}

impl AudioSource for WavSource {
    fn start(&mut self, output: AudioProducer, events: SourceEvents) -> Result<SourceInfo> {
        if self.worker.is_some() {
            return Err(Error::Engine("WAV replay is already running".into()));
        }
        let mut reader = WavReader::open(&self.path).map_err(wav_error)?;
        if reader.spec() != self.spec || reader.duration() != self.file_frames {
            return Err(Error::Engine(
                "WAV changed since replay was configured".into(),
            ));
        }
        let position = self.position.load(Ordering::Acquire);
        let already_read = (u128::from(position) * u128::from(self.spec.sample_rate)
            / u128::from(SAMPLE_RATE))
        .min(u128::from(u32::MAX)) as u32;
        let start = self
            .start_frame
            .saturating_add(already_read)
            .min(self.end_frame);
        reader.seek(start).map_err(Error::Io)?;
        let info = self.source_info();
        let assembler = PreparedFrames {
            output,
            events,
            normalizer: LevelNormalizer::new(self.audio.clone()),
            gaps: self.gaps.clone(),
            position: Arc::clone(&self.position),
            relative: position,
            frame_start: StreamTime::ZERO,
            samples: [0.0; FRAME_SAMPLES],
            filled: 0,
            all_gap: true,
            pace: self.pace,
            silent_samples: 0,
            source_state: None,
        };
        let mut assembler = assembler;
        assembler.frame_start = assembler.output.base_time;
        self.cancelled = Some(Arc::clone(&assembler.output.cancelled));
        let error = Arc::clone(&self.error);
        if let Ok(mut error) = error.lock() {
            *error = None;
        }
        let remaining = u64::from(self.end_frame - start);
        let spec = self.spec;
        self.worker = Some(
            thread::Builder::new()
                .name("wav-replay".into())
                .spawn(move || {
                    let result = replay(reader, spec, remaining, &mut assembler);
                    if let Err(failure) = &result {
                        if !matches!(failure, Error::Stopped) {
                            tracing::error!(message = %failure, "WAV replay failed");
                            if let Ok(mut error) = error.lock() {
                                *error = Some(failure.to_string());
                            }
                        }
                    }
                    match result {
                        Err(Error::Stopped) => Ok(()),
                        other => other,
                    }
                })
                .map_err(Error::Io)?,
        );
        Ok(info)
    }

    fn stop(&mut self) {
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Release);
        }
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                if let Ok(mut error) = self.error.lock() {
                    *error = Some("WAV replay worker stopped unexpectedly".into());
                }
            }
        }
    }
}

impl Drop for WavSource {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Decode a finite benchmark/ASR clip with the same downmix/resampler path as
/// replay. Returns exact 16 kHz samples, without normalization or frame padding.
pub fn read_wav_mono16(path: impl AsRef<Path>) -> Result<Vec<f32>> {
    let source = WavSource::new(path.as_ref(), Pace::AsFastAsPossible)?;
    let mut reader = WavReader::open(path).map_err(wav_error)?;
    let mut remaining = u64::from(source.file_frames);
    let channels = usize::from(source.spec.channels);
    let packet_frames = (4_096 / channels).max(1);
    let mut interleaved = Vec::with_capacity(packet_frames * channels);
    let mut mono = Vec::with_capacity(packet_frames);
    let mut output = Vec::new();
    let mut resampler = MonoResampler::new(source.spec.sample_rate)?;
    while remaining != 0 {
        let count = (remaining as usize).min(packet_frames);
        read_packet(&mut reader, source.spec, count, &mut interleaved)?;
        mono.clear();
        downmix_interleaved(&interleaved, source.spec.channels, &mut mono)?;
        resampler.push(&mono, &mut output)?;
        remaining -= count as u64;
    }
    resampler.finish(&mut output)?;
    Ok(output)
}

fn wav_error(error: hound::Error) -> Error {
    match error {
        hound::Error::IoError(error) => Error::Io(error),
        other => Error::Engine(format!("Could not decode WAV: {other}")),
    }
}

fn read_packet(
    reader: &mut WavReader<BufReader<File>>,
    spec: WavSpec,
    count: usize,
    interleaved: &mut Vec<f32>,
) -> Result<()> {
    let channels = usize::from(spec.channels);
    interleaved.clear();
    match spec.sample_format {
        SampleFormat::Float => {
            for sample in reader.samples::<f32>().take(count * channels) {
                let sample = sample.map_err(wav_error)?;
                if !sample.is_finite() {
                    return Err(Error::Engine("WAV contains a non-finite sample".into()));
                }
                interleaved.push(sample);
            }
        }
        SampleFormat::Int => {
            let full_scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
            for sample in reader.samples::<i32>().take(count * channels) {
                interleaved.push((f64::from(sample.map_err(wav_error)?) / full_scale) as f32);
            }
        }
    }
    if interleaved.len() != count * channels {
        return Err(Error::Engine(
            "WAV ended before its declared sample count".into(),
        ));
    }
    Ok(())
}

fn replay(
    mut reader: WavReader<BufReader<File>>,
    spec: WavSpec,
    mut remaining: u64,
    assembler: &mut PreparedFrames,
) -> Result<()> {
    let mut resampler = MonoResampler::new(spec.sample_rate)?;
    let channels = usize::from(spec.channels);
    let packet_frames = (4_096 / channels).max(1);
    let mut interleaved = Vec::with_capacity(packet_frames * channels);
    let mut mono = Vec::with_capacity(packet_frames);
    let mut prepared = Vec::new();
    while remaining != 0 {
        if assembler.output.cancelled.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        let count = (remaining as usize).min(packet_frames);
        read_packet(&mut reader, spec, count, &mut interleaved)?;
        mono.clear();
        downmix_interleaved(&interleaved, spec.channels, &mut mono)?;
        prepared.clear();
        resampler.push(&mono, &mut prepared)?;
        assembler.push(&prepared)?;
        remaining -= count as u64;
    }
    prepared.clear();
    resampler.finish(&mut prepared)?;
    assembler.push(&prepared)?;
    assembler.finish()
}

struct PreparedFrames {
    output: AudioProducer,
    events: SourceEvents,
    normalizer: LevelNormalizer,
    gaps: Vec<InjectedGap>,
    position: Arc<AtomicU64>,
    relative: u64,
    frame_start: StreamTime,
    samples: [f32; FRAME_SAMPLES],
    filled: usize,
    all_gap: bool,
    pace: Pace,
    silent_samples: u64,
    source_state: Option<SourceStateKind>,
}

impl PreparedFrames {
    fn push(&mut self, input: &[f32]) -> Result<()> {
        for sample in input {
            let at = self.relative.saturating_add(self.filled as u64);
            let gap = self.gaps.iter().any(|gap| {
                at >= gap.start.samples()
                    && at < gap.start.samples().saturating_add(gap.duration.samples())
            });
            self.samples[self.filled] = if gap { 0.0 } else { *sample };
            self.all_gap &= gap;
            self.filled += 1;
            if self.filled == FRAME_SAMPLES {
                self.emit(FRAME_SAMPLES)?;
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        if self.filled == 0 {
            return Ok(());
        }
        let valid = self.filled;
        self.samples[valid..].fill(0.0);
        self.emit(valid)
    }

    fn emit(&mut self, valid: usize) -> Result<()> {
        self.normalizer.process(&mut self.samples);
        if self.pace == Pace::RealTime {
            self.output.wait_until_frame_end(self.frame_start)?;
        }
        let flags = if self.all_gap {
            FrameFlags::GAP_FILLED
        } else {
            FrameFlags::EMPTY
        };
        self.output.send(AudioFrame {
            t0: self.frame_start,
            samples: self.samples,
            flags,
        })?;
        self.relative = self.relative.saturating_add(valid as u64);
        self.position.store(self.relative, Ordering::Release);
        let state = if self.samples.iter().any(|sample| *sample != 0.0) {
            self.silent_samples = 0;
            Some(SourceStateKind::Playing)
        } else {
            self.silent_samples = self.silent_samples.saturating_add(FRAME_SAMPLES as u64);
            (self.silent_samples >= 3 * SAMPLE_RATE).then_some(SourceStateKind::Silent)
        };
        if let Some(state) = state {
            if self.source_state != Some(state) {
                self.events.publish(PipelineEvent::SourceState {
                    state,
                    detail: None,
                });
                self.source_state = Some(state);
            }
        }
        self.frame_start = StreamTime(
            self.frame_start
                .samples()
                .saturating_add(FRAME_SAMPLES as u64),
        );
        self.filled = 0;
        self.all_gap = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::{
        bus::EventBus,
        config::Config,
        fakes::{FakeAsr, FakeTranslator, FakeVad},
        pipeline::Pipeline,
    };
    use std::{fs, sync::atomic::AtomicUsize};

    static TEMP_ID: AtomicUsize = AtomicUsize::new(0);
    struct FixtureDir {
        path: PathBuf,
        root: PathBuf,
    }
    impl FixtureDir {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap();
            let path = root.join("target/tmp").join(format!(
                "wav-test-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path, root }
        }
        fn tone(&self, rate: u32, channels: u16, seconds: f64) -> PathBuf {
            let path = self.path.join("tone.wav");
            let mut writer = hound::WavWriter::create(
                &path,
                WavSpec {
                    channels,
                    sample_rate: rate,
                    bits_per_sample: 32,
                    sample_format: SampleFormat::Float,
                },
            )
            .unwrap();
            for index in 0..(seconds * f64::from(rate)) as usize {
                let sample =
                    0.25 * (std::f32::consts::TAU * 1_000.0 * index as f32 / rate as f32).sin();
                for _ in 0..channels {
                    writer.write_sample(sample).unwrap();
                }
            }
            writer.finalize().unwrap();
            path
        }
    }
    impl Drop for FixtureDir {
        fn drop(&mut self) {
            if let Ok(path) = self.path.canonicalize() {
                if path.starts_with(self.root.join("target/tmp")) {
                    let _ = fs::remove_dir_all(path);
                }
            }
        }
    }
    fn no_normalize() -> AudioConfig {
        AudioConfig {
            normalize: false,
            ..AudioConfig::default()
        }
    }

    #[test]
    fn wav_replay_runs_through_fake_pipeline_end_to_end() {
        let fixtures = FixtureDir::new();
        let path = fixtures.tone(48_000, 2, 1.5);
        let source = WavSource::new(path, Pace::AsFastAsPossible)
            .unwrap()
            .with_audio_config(no_normalize());
        let errors = source.error_handle();
        let bus = EventBus::default();
        let events = bus.subscribe(128);
        let config = Config {
            join: lt_core::config::JoinConfig {
                hold_max_chars: 0,
                ..lt_core::config::JoinConfig::default()
            },
            ..Config::default()
        };
        let mut pipeline = Pipeline::start_with_bus(
            config,
            Box::new(source),
            Box::new(FakeVad::from_energy(0.005)),
            Box::new(FakeAsr::new(vec!["我们今天一起谈论这个问题。".into()])),
            Box::new(FakeTranslator::default()),
            bus,
        )
        .unwrap();
        pipeline.wait().unwrap();
        let events: Vec<_> = events.try_iter().collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, PipelineEvent::AsrFinal { .. }))
                .count(),
            1
        );
        assert_eq!(events.iter().filter(|event| matches!(event, PipelineEvent::TranslationFinal { text, .. } if text == "Hello.")).count(), 1);
        assert!(errors.lock().unwrap().is_none());
    }
}
