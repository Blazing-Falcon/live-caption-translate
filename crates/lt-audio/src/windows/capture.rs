//! Native WASAPI packet reader and its prepared-frame adapter.
//!
//! The published wasapi reader copies SILENT pointers and logs in GetBuffer;
//! this reader uses the same public Windows interfaces, without those operations.
use super::devices::{create_enumerator, native_error, MtaGuard, NotificationGuard};
use crate::{normalize::LevelNormalizer, resample::MonoResampler};
use crossbeam_channel::{Receiver, Sender};
use lt_core::{
    config::AudioConfig,
    error::{Error, Result},
    events::{PipelineEvent, SourceStateKind},
    segment::FrameFlags,
    source::{AudioFrame, AudioProducer, SourceEvents},
    types::{SourceInfo, StreamTime},
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    core::{w, GUID, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_TIMEOUT},
        Media::Audio::{
            IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
            AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
            AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
            WAVEFORMATEXTENSIBLE,
        },
        System::{
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            Threading::{
                AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
                WaitForSingleObject,
            },
        },
    },
};

const PCM_GUID: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const FLOAT_GUID: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const NATIVE_WAIT_MS: u32 = 50;
fn ring_packets(sample_rate: u32) -> usize {
    (u64::from(sample_rate) * 2).div_ceil(512) as usize + 1
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Encoding {
    Pcm8,
    Pcm16,
    Pcm24,
    Pcm32,
    Float32,
    Float64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FormatIdentity {
    sample_rate: u32,
    channels: u16,
    bits: u16,
    valid_bits: u16,
    block_align: u16,
    channel_mask: u32,
    encoding: Encoding,
}

#[derive(Clone)]
pub(crate) struct CaptureFormat {
    wave: WAVEFORMATEXTENSIBLE,
    identity: FormatIdentity,
    weights: [f32; 32],
}
impl CaptureFormat {
    pub(crate) fn float_stereo_48k() -> Self {
        let wave = WAVEFORMATEX {
            wFormatTag: 3,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 384_000,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // This known valid format needs no native calls or allocated buffers.
        Self::from_wave(wave, None).expect("fixed process format is valid")
    }
    /// Copy the owned native mix format before its CoTaskMem allocation is freed.
    /// SAFETY: pointer must reference a Windows-owned valid WAVEFORMATEX, and its
    /// complete extension when wFormatTag is WAVE_FORMAT_EXTENSIBLE.
    pub(crate) unsafe fn copy_mix_format(pointer: *const WAVEFORMATEX) -> Result<Self> {
        if pointer.is_null() {
            return Err(Error::Engine("WASAPI returned a null mix format".into()));
        }
        let wave = unsafe { pointer.read_unaligned() };
        let extension = if wave.wFormatTag == 0xfffe {
            if wave.cbSize < 22 {
                return Err(Error::Engine("Truncated extensible audio format".into()));
            }
            Some(unsafe { pointer.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned() })
        } else {
            None
        };
        Self::from_wave(wave, extension)
    }
    fn from_wave(wave: WAVEFORMATEX, extension: Option<WAVEFORMATEXTENSIBLE>) -> Result<Self> {
        let channels = wave.nChannels;
        let rate = wave.nSamplesPerSec;
        let bits = wave.wBitsPerSample;
        if channels == 0 || channels > 32 || !(8_000..=384_000).contains(&rate) {
            return Err(Error::Engine(
                "Unsupported endpoint channel count or sample rate".into(),
            ));
        }
        let (tag, valid_bits, mask) = match extension {
            Some(ext) => {
                let subtype = ext.SubFormat;
                let tag = if subtype == PCM_GUID {
                    1
                } else if subtype == FLOAT_GUID {
                    3
                } else {
                    return Err(Error::Engine("Unsupported extensible audio subtype".into()));
                };
                (
                    tag,
                    unsafe { ext.Samples.wValidBitsPerSample },
                    ext.dwChannelMask,
                )
            }
            None => (wave.wFormatTag, bits, 0),
        };
        let encoding = match (tag, bits) {
            (1, 8) => Encoding::Pcm8,
            (1, 16) => Encoding::Pcm16,
            (1, 24) => Encoding::Pcm24,
            (1, 32) => Encoding::Pcm32,
            (3, 32) => Encoding::Float32,
            (3, 64) => Encoding::Float64,
            _ => {
                return Err(Error::Engine(
                    "Unsupported PCM or float audio container".into(),
                ))
            }
        };
        if valid_bits == 0 || valid_bits > bits || wave.nBlockAlign < channels * (bits / 8) {
            return Err(Error::Engine(
                "Invalid audio frame alignment or valid-bit count".into(),
            ));
        }
        let mut weights = [0.0; 32];
        if channels == 1 {
            weights[0] = 1.0;
        } else if mask != 0 {
            let mut channel = 0usize;
            for bit in 0..32 {
                if mask & (1 << bit) == 0 {
                    continue;
                }
                if channel == channels as usize {
                    break;
                }
                weights[channel] = match bit {
                    0 | 1 => 0.5,
                    2 => std::f32::consts::FRAC_1_SQRT_2,
                    _ => 0.0,
                };
                channel += 1;
            }
            if channel != channels as usize {
                return Err(Error::Engine(
                    "Audio channel mask disagrees with channel count".into(),
                ));
            }
        } else {
            weights[0] = 0.5;
            weights[1] = 0.5;
            if channels == 6 || channels == 8 {
                weights[2] = std::f32::consts::FRAC_1_SQRT_2;
            }
        }
        let identity = FormatIdentity {
            sample_rate: rate,
            channels,
            bits,
            valid_bits,
            block_align: wave.nBlockAlign,
            channel_mask: mask,
            encoding,
        };
        let mut owned = extension.unwrap_or_default();
        owned.Format = wave;
        if extension.is_some() {
            owned.Format.cbSize = 22;
        }
        Ok(Self {
            wave: owned,
            identity,
            weights,
        })
    }
    pub(crate) fn sample_rate(&self) -> u32 {
        self.identity.sample_rate
    }
    pub(crate) fn channels(&self) -> u16 {
        self.identity.channels
    }
    fn as_ptr(&self) -> *const WAVEFORMATEX {
        // WAVEFORMATEXTENSIBLE begins with WAVEFORMATEX by the documented ABI.
        std::ptr::from_ref(&self.wave).cast()
    }
    /// SAFETY: data must cover one complete native frame. No aligned references
    /// are formed: packed 24-bit and unaligned integer/float packets are valid.
    unsafe fn decode_mono(&self, data: *const u8) -> f32 {
        let bytes = usize::from(self.identity.bits / 8);
        let mut sum = 0.0;
        for channel in 0..usize::from(self.identity.channels) {
            let weight = self.weights[channel];
            if weight == 0.0 {
                continue;
            }
            let pointer = unsafe { data.add(channel * bytes) };
            let value = match self.identity.encoding {
                Encoding::Pcm8 => (f32::from(unsafe { *pointer }) - 128.0) / 128.0,
                Encoding::Pcm16 => {
                    f32::from(unsafe { pointer.cast::<i16>().read_unaligned() }) / 32768.0
                }
                Encoding::Pcm24 => {
                    let raw = i32::from(unsafe { *pointer })
                        | (i32::from(unsafe { *pointer.add(1) }) << 8)
                        | (i32::from(unsafe { *pointer.add(2) }) << 16);
                    ((raw << 8) >> 8) as f32 / 8_388_608.0
                }
                Encoding::Pcm32 => {
                    (unsafe { pointer.cast::<i32>().read_unaligned() }) as f32 / 2_147_483_648.0
                }
                Encoding::Float32 => unsafe { pointer.cast::<f32>().read_unaligned() },
                Encoding::Float64 => (unsafe { pointer.cast::<f64>().read_unaligned() }) as f32,
            };
            if value.is_finite() {
                sum += value * weight;
            }
        }
        sum
    }
}

pub(crate) struct OpenedClient {
    pub client: IAudioClient,
    pub format: CaptureFormat,
    pub info: SourceInfo,
    pub identity: String,
    pub autoconvert: bool,
    pub buffer_duration_hns: i64,
}

#[derive(Clone, Copy)]
pub(crate) struct CapturePacket {
    pub generation: u64,
    pub frames: u16,
    pub samples: [f32; 512],
    pub device_position: u64,
    pub qpc_100ns: u64,
    pub silent: bool,
    pub timestamp_error: bool,
    pub data_discontinuity: bool,
}
impl Default for CapturePacket {
    fn default() -> Self {
        Self {
            generation: 0,
            frames: 0,
            samples: [0.0; 512],
            device_position: 0,
            qpc_100ns: 0,
            silent: true,
            timestamp_error: false,
            data_discontinuity: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PacketReadError {
    NullData,
    LengthOverflow,
}

#[derive(Default)]
struct PacketAssembler {
    packet: CapturePacket,
}
impl PacketAssembler {
    fn flush(&mut self, emit: &mut impl FnMut(CapturePacket)) {
        if self.packet.frames != 0 {
            emit(self.packet);
            self.packet = CapturePacket::default();
        }
    }
    /// SAFETY: non-SILENT pointer covers frames * native block alignment bytes.
    /// SILENT deliberately accepts null: Windows does not define its contents.
    #[allow(clippy::too_many_arguments)] // Mirrors the native packet's fixed fields.
    unsafe fn append(
        &mut self,
        format: &CaptureFormat,
        data: *const u8,
        frames: u32,
        flags: u32,
        position: u64,
        qpc: u64,
        generation: u64,
        emit: &mut impl FnMut(CapturePacket),
    ) -> std::result::Result<(), PacketReadError> {
        let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
        let timestamp_error = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0;
        let discontinuity = flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0;
        if !silent && data.is_null() && frames != 0 {
            return Err(PacketReadError::NullData);
        }
        let alignment = usize::from(format.identity.block_align);
        let byte_length = (frames as usize)
            .checked_mul(alignment)
            .ok_or(PacketReadError::LengthOverflow)?;
        if byte_length > isize::MAX as usize {
            return Err(PacketReadError::LengthOverflow);
        }
        if self.packet.frames != 0 {
            let expected = self
                .packet
                .device_position
                .saturating_add(u64::from(self.packet.frames));
            let expected_qpc = self.packet.qpc_100ns.saturating_add(samples_to_hns(
                u64::from(self.packet.frames),
                format.sample_rate(),
            ));
            let qpc_gap = !timestamp_error
                && !self.packet.timestamp_error
                && expected_qpc.abs_diff(qpc) > samples_to_hns(1, format.sample_rate()).max(1);
            if expected != position
                || timestamp_error != self.packet.timestamp_error
                || discontinuity
                || qpc_gap
                || self.packet.generation != generation
            {
                self.flush(emit);
            }
        }
        for offset in 0..frames {
            if self.packet.frames == 0 {
                self.packet.generation = generation;
                self.packet.device_position = position.saturating_add(u64::from(offset));
                self.packet.qpc_100ns =
                    qpc.saturating_add(samples_to_hns(u64::from(offset), format.sample_rate()));
                self.packet.timestamp_error = timestamp_error;
                self.packet.data_discontinuity = discontinuity && offset == 0;
            }
            self.packet.silent &= silent;
            let value = if silent {
                0.0
            } else {
                unsafe { format.decode_mono(data.add(offset as usize * alignment)) }
            };
            self.packet.samples[usize::from(self.packet.frames)] = value;
            self.packet.frames += 1;
            if self.packet.frames == 512 {
                self.flush(emit);
            }
        }
        Ok(())
    }
}

pub(crate) fn samples_to_hns(samples: u64, rate: u32) -> u64 {
    ((u128::from(samples) * 10_000_000) / u128::from(rate)).min(u128::from(u64::MAX)) as u64
}
pub(crate) fn hns_to_samples(hns: u64, rate: u32) -> u64 {
    ((u128::from(hns) * u128::from(rate)) / 10_000_000).min(u128::from(u64::MAX)) as u64
}

pub(crate) struct QpcClock {
    frequency: u64,
}
impl QpcClock {
    pub(crate) fn new() -> Result<Self> {
        let mut frequency = 0;
        unsafe { QueryPerformanceFrequency(&mut frequency) }.map_err(native_error)?;
        if frequency <= 0 {
            return Err(Error::Engine(
                "Invalid performance-counter frequency".into(),
            ));
        }
        Ok(Self {
            frequency: frequency as u64,
        })
    }
    pub(crate) fn now(&self) -> Result<u64> {
        let mut counter = 0;
        unsafe { QueryPerformanceCounter(&mut counter) }.map_err(native_error)?;
        Ok(
            ((counter.max(0) as u128 * 10_000_000) / u128::from(self.frequency))
                .min(u128::from(u64::MAX)) as u64,
        )
    }
}
pub(crate) fn qpc_100ns() -> Result<u64> {
    QpcClock::new()?.now()
}

struct EventHandle(HANDLE);
impl Drop for EventHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}
struct MmcssGuard(HANDLE);
impl MmcssGuard {
    fn new() -> Result<Self> {
        let mut index = 0;
        unsafe { AvSetMmThreadCharacteristicsW(w!("Audio"), &mut index) }
            .map(Self)
            .map_err(native_error)
    }
}
impl Drop for MmcssGuard {
    fn drop(&mut self) {
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.0) };
    }
}

struct ActiveClient {
    opened: OpenedClient,
    capture: IAudioCaptureClient,
    event: EventHandle,
    started_qpc_100ns: u64,
}
impl ActiveClient {
    fn start(opened: OpenedClient, clock: &QpcClock) -> Result<Self> {
        let mut flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
        if opened.autoconvert {
            flags |= AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        }
        unsafe {
            opened.client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                opened.buffer_duration_hns,
                0,
                opened.format.as_ptr(),
                None,
            )
        }
        .map_err(native_error)?;
        let event = EventHandle(
            unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map_err(native_error)?,
        );
        unsafe { opened.client.SetEventHandle(event.0) }.map_err(native_error)?;
        let capture =
            unsafe { opened.client.GetService::<IAudioCaptureClient>() }.map_err(native_error)?;
        let started_qpc_100ns = clock.now()?;
        unsafe { opened.client.Start() }.map_err(native_error)?;
        Ok(Self {
            opened,
            capture,
            event,
            started_qpc_100ns,
        })
    }
    fn drain(
        &self,
        generation: u64,
        assembler: &mut PacketAssembler,
        producer: &mut Producer<CapturePacket>,
        overflows: &AtomicU64,
    ) -> Result<()> {
        let mut emit = |packet: CapturePacket| {
            if producer.push(packet).is_err() {
                overflows.fetch_add(u64::from(packet.frames), Ordering::Relaxed);
            }
        };
        loop {
            let available = unsafe { self.capture.GetNextPacketSize() }.map_err(native_error)?;
            if available == 0 {
                break;
            }
            let mut data = std::ptr::null_mut();
            let (mut frames, mut flags, mut position, mut qpc) = (0, 0, 0, 0);
            unsafe {
                self.capture.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut position),
                    Some(&mut qpc),
                )
            }
            .map_err(native_error)?;
            // Release each native buffer even when defensive decoding rejects it.
            // append, emit and ReleaseBuffer contain no log, lock or heap allocation.
            let decoded = unsafe {
                assembler.append(
                    &self.opened.format,
                    data,
                    frames,
                    flags,
                    position,
                    qpc,
                    generation,
                    &mut emit,
                )
            };
            let released = unsafe { self.capture.ReleaseBuffer(frames) };
            released.map_err(native_error)?;
            if let Err(error) = decoded {
                return Err(Error::Engine(format!(
                    "Invalid native capture packet: {error:?}"
                )));
            }
        }
        // Retain a partial mono chunk across event wakes. Tiny native packets
        // cannot consume one queue slot each; a confirmed timeout flushes it.
        Ok(())
    }
}
impl Drop for ActiveClient {
    fn drop(&mut self) {
        let _ = unsafe { self.opened.client.Stop() };
    }
}

#[derive(Clone, Copy)]
pub(crate) struct WorkerOptions {
    pub watch_devices: bool,
    pub unavailable_state: SourceStateKind,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum NativeNotice {
    Ready {
        generation: u64,
        format: CaptureFormat,
        info: SourceInfo,
        opened_qpc_100ns: u64,
        consumer: Consumer<CapturePacket>,
    },
    Unavailable {
        state: SourceStateKind,
        detail: String,
    },
}

type OpenFactory = Box<dyn FnMut(&IMMDeviceEnumerator, &AtomicBool) -> Result<OpenedClient> + Send>;
pub(crate) struct NativeWorker {
    pub consumer: Consumer<CapturePacket>,
    pub notices: Receiver<NativeNotice>,
    pub overflows: Arc<AtomicU64>,
    pub cancelled: Arc<AtomicBool>,
    pub join: Option<JoinHandle<Result<()>>>,
    pub start_qpc_100ns: u64,
    pub timeout_qpc_100ns: Arc<AtomicU64>,
}
impl NativeWorker {
    pub(crate) fn spawn(
        factory: OpenFactory,
        options: WorkerOptions,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self> {
        let start_qpc_100ns = qpc_100ns()?;
        // The worker allocates the real ring only after learning the endpoint
        // rate. This empty placeholder never carries native audio.
        let (producer, consumer) = RingBuffer::new(1);
        let (notice_sender, notices) = crossbeam_channel::bounded(16);
        let overflows = Arc::new(AtomicU64::new(0));
        let worker_cancelled = cancelled.clone();
        let worker_overflows = overflows.clone();
        let timeout_qpc_100ns = Arc::new(AtomicU64::new(start_qpc_100ns));
        let worker_timeout = timeout_qpc_100ns.clone();
        let join = thread::Builder::new()
            .name("lt-wasapi-capture".into())
            .spawn(move || {
                native_loop(
                    factory,
                    options,
                    worker_cancelled,
                    producer,
                    notice_sender,
                    worker_overflows,
                    worker_timeout,
                )
            })?;
        Ok(Self {
            consumer,
            notices,
            overflows,
            cancelled,
            join: Some(join),
            start_qpc_100ns,
            timeout_qpc_100ns,
        })
    }
}
impl Drop for NativeWorker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Default)]
struct ReopenBackoff {
    attempts: u32,
}
impl ReopenBackoff {
    fn next(&mut self) -> Duration {
        let delay = match self.attempts {
            0 => 500,
            1 => 1000,
            _ => 2000,
        };
        self.attempts = self.attempts.saturating_add(1);
        Duration::from_millis(delay)
    }
    fn reset(&mut self) {
        self.attempts = 0;
    }
}
fn cancelled_wait(cancelled: &AtomicBool, duration: Duration) -> bool {
    let until = Instant::now() + duration;
    while !cancelled.load(Ordering::Acquire) {
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        thread::sleep(remaining.min(Duration::from_millis(20)));
    }
    true
}
fn send_notice(
    sender: &Sender<NativeNotice>,
    cancelled: &AtomicBool,
    notice: NativeNotice,
) -> Result<()> {
    let mut notice = notice;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        match sender.send_timeout(notice, Duration::from_millis(20)) {
            Ok(()) => return Ok(()),
            Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => notice = returned,
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => {
                return Err(Error::Stopped)
            }
        }
    }
}
fn flush_pending(
    assembler: &mut PacketAssembler,
    producer: &mut Producer<CapturePacket>,
    overflows: &AtomicU64,
) {
    assembler.flush(&mut |packet: CapturePacket| {
        if producer.push(packet).is_err() {
            overflows.fetch_add(u64::from(packet.frames), Ordering::Relaxed);
        }
    });
}
fn native_loop(
    mut factory: OpenFactory,
    options: WorkerOptions,
    cancelled: Arc<AtomicBool>,
    mut producer: Producer<CapturePacket>,
    notices: Sender<NativeNotice>,
    overflows: Arc<AtomicU64>,
    timeout_qpc_100ns: Arc<AtomicU64>,
) -> Result<()> {
    let _mta = MtaGuard::new()?;
    let enumerator = create_enumerator()?;
    let clock = QpcClock::new()?;
    let dirty = Arc::new(AtomicU64::new(0));
    let _notification = if options.watch_devices {
        Some(NotificationGuard::new(&enumerator, dirty.clone())?)
    } else {
        None
    };
    let _mmcss = MmcssGuard::new()?;
    let mut backoff = ReopenBackoff::default();
    let mut generation = 0u64;
    let mut observed_dirty = dirty.load(Ordering::Acquire);
    let mut active: Option<ActiveClient> = None;
    let mut assembler = PacketAssembler::default();
    while !cancelled.load(Ordering::Acquire) {
        if active.is_none() {
            let opened = factory(&enumerator, &cancelled)
                .and_then(|opened| ActiveClient::start(opened, &clock));
            match opened {
                Ok(client) => {
                    generation = generation.saturating_add(1);
                    let (next_producer, consumer) =
                        RingBuffer::new(ring_packets(client.opened.format.sample_rate()));
                    let notice = NativeNotice::Ready {
                        generation,
                        format: client.opened.format.clone(),
                        info: client.opened.info.clone(),
                        opened_qpc_100ns: client.started_qpc_100ns,
                        consumer,
                    };
                    send_notice(&notices, &cancelled, notice)?;
                    producer = next_producer;
                    active = Some(client);
                    backoff.reset();
                    assembler = PacketAssembler::default();
                    observed_dirty = dirty.load(Ordering::Acquire);
                }
                Err(Error::Stopped) => return Ok(()),
                Err(error) => {
                    send_notice(
                        &notices,
                        &cancelled,
                        NativeNotice::Unavailable {
                            state: options.unavailable_state,
                            detail: error.to_string(),
                        },
                    )?;
                    if cancelled_wait(&cancelled, backoff.next()) {
                        return Ok(());
                    }
                    continue;
                }
            }
        }
        let current_dirty = dirty.load(Ordering::Acquire);
        if options.watch_devices && current_dirty != observed_dirty {
            observed_dirty = current_dirty;
            match factory(&enumerator, &cancelled) {
                Ok(candidate) => {
                    let current = &active.as_ref().expect("opened above").opened;
                    if candidate.identity != current.identity
                        || candidate.format.identity != current.format.identity
                    {
                        // Drop old stream before initialize; DSP boundary comes with Ready.
                        flush_pending(&mut assembler, &mut producer, &overflows);
                        active = None;
                        match ActiveClient::start(candidate, &clock) {
                            Ok(client) => {
                                generation = generation.saturating_add(1);
                                let (next_producer, consumer) = RingBuffer::new(ring_packets(
                                    client.opened.format.sample_rate(),
                                ));
                                send_notice(
                                    &notices,
                                    &cancelled,
                                    NativeNotice::Ready {
                                        generation,
                                        format: client.opened.format.clone(),
                                        info: client.opened.info.clone(),
                                        opened_qpc_100ns: client.started_qpc_100ns,
                                        consumer,
                                    },
                                )?;
                                producer = next_producer;
                                active = Some(client);
                                assembler = PacketAssembler::default();
                                backoff.reset();
                            }
                            Err(error) => {
                                send_notice(
                                    &notices,
                                    &cancelled,
                                    NativeNotice::Unavailable {
                                        state: options.unavailable_state,
                                        detail: error.to_string(),
                                    },
                                )?;
                                if cancelled_wait(&cancelled, backoff.next()) {
                                    return Ok(());
                                }
                                continue;
                            }
                        }
                    }
                }
                Err(Error::Stopped) => return Ok(()),
                Err(error) => {
                    flush_pending(&mut assembler, &mut producer, &overflows);
                    active = None;
                    send_notice(
                        &notices,
                        &cancelled,
                        NativeNotice::Unavailable {
                            state: options.unavailable_state,
                            detail: error.to_string(),
                        },
                    )?;
                    if cancelled_wait(&cancelled, backoff.next()) {
                        return Ok(());
                    }
                    continue;
                }
            }
        }
        let client = active.as_ref().expect("active or retry above");
        let wait = unsafe { WaitForSingleObject(client.event.0, NATIVE_WAIT_MS) };
        if cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let result = if wait == WAIT_FAILED {
            Err(native_error(windows::core::Error::from_thread()))
        } else {
            client.drain(generation, &mut assembler, &mut producer, &overflows)
        };
        if wait == WAIT_TIMEOUT && result.is_ok() {
            flush_pending(&mut assembler, &mut producer, &overflows);
            // A packet's timestamp names its first sample; wait a full capture
            // interval before committing wall-time gaps so buffered audio wins.
            timeout_qpc_100ns.store(clock.now()?.saturating_sub(500_000), Ordering::Release);
        }
        if let Err(error) = result {
            flush_pending(&mut assembler, &mut producer, &overflows);
            active = None;
            send_notice(
                &notices,
                &cancelled,
                NativeNotice::Unavailable {
                    state: options.unavailable_state,
                    detail: error.to_string(),
                },
            )?;
            if cancelled_wait(&cancelled, backoff.next()) {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Shared by every selected root so source activation/restart is not a new time 0.
#[derive(Clone, Copy)]
pub(crate) struct CaptureEpoch {
    pub qpc_100ns: u64,
    pub stream_time: StreamTime,
}
impl CaptureEpoch {
    fn stream_at(self, qpc: u64) -> StreamTime {
        StreamTime(
            self.stream_time
                .samples()
                .saturating_add(hns_to_samples(qpc.saturating_sub(self.qpc_100ns), 16_000)),
        )
    }
    fn raw_at(self, qpc: u64, rate: u32) -> u64 {
        hns_to_samples(qpc.saturating_sub(self.qpc_100ns), rate)
    }
}

/// Invalid QPC metadata falls back to the last trustworthy device-frame anchor.
#[derive(Default)]
struct PacketClock {
    anchor: Option<(u64, u64)>,
    next_raw: u64,
}
impl PacketClock {
    fn packet_t0(
        &mut self,
        packet: &CapturePacket,
        rate: u32,
        opened_qpc: u64,
        epoch: CaptureEpoch,
    ) -> u64 {
        let predicted = self.anchor.and_then(|(position, raw)| {
            packet
                .device_position
                .checked_sub(position)
                .map(|delta| raw.saturating_add(delta))
        });
        let raw = if packet.timestamp_error {
            predicted.unwrap_or_else(|| self.next_raw.max(epoch.raw_at(opened_qpc, rate)))
        } else {
            let mapped = epoch.raw_at(packet.qpc_100ns, rate);
            // QPC is quantized to100ns. Flooring an exact512-frame48k chunk
            // through that domain maps it to511, creating false overlap/gaps.
            // One raw sample covers that quantization (even384k=0.0384 samples)
            // plus the initial integer-grid phase. Larger differences remain
            // real timestamp holes; device-frame advances themselves are exact.
            let raw = match predicted {
                Some(predicted) if predicted.abs_diff(mapped) <= 1 => predicted,
                _ => mapped,
            };
            self.anchor = Some((packet.device_position, raw));
            raw
        };
        self.next_raw = raw.saturating_add(u64::from(packet.frames));
        raw
    }
}

pub(crate) fn run_adapter(
    worker: NativeWorker,
    output: AudioProducer,
    events: SourceEvents,
    audio: AudioConfig,
) -> Result<()> {
    let epoch = CaptureEpoch {
        qpc_100ns: worker.start_qpc_100ns,
        stream_time: output.base_time,
    };
    run_adapter_at(worker, output, events, audio, epoch)
}

// Adapter implementation below consumes the portable MonoTimeline seam; native
// buffers and COM never leave the capture worker. All allocation is permitted here.
pub(crate) fn run_adapter_at(
    mut worker: NativeWorker,
    mut output: AudioProducer,
    events: SourceEvents,
    audio: AudioConfig,
    epoch: CaptureEpoch,
) -> Result<()> {
    let pipeline_cancelled = output.cancelled.clone();
    // Source.stop must wake a blocked prepared-channel send even when a live
    // pipeline is merely switching sources and its global stop flag stays false.
    output.cancelled = worker.cancelled.clone();
    let result = adapter_loop(
        &mut worker,
        &output,
        &events,
        audio,
        epoch,
        &pipeline_cancelled,
    );
    worker.cancelled.store(true, Ordering::Release);
    if let Some(join) = worker.join.take() {
        let native = join
            .join()
            .map_err(|_| Error::Engine("WASAPI capture worker panicked".into()))?;
        if result.is_ok() && !matches!(native, Ok(()) | Err(Error::Stopped)) {
            return native;
        }
    }
    match result {
        Err(Error::Stopped) => Ok(()),
        other => other,
    }
}

struct PreparedFrames {
    resampler: MonoResampler,
    normalizer: LevelNormalizer,
    converted: Vec<f32>,
    frame: [f32; 512],
    filled: usize,
    frame_flags: FrameFlags,
    next_time: StreamTime,
    mark_discontinuity: bool,
}
impl PreparedFrames {
    fn new(rate: u32, audio: AudioConfig, start: StreamTime, discontinuity: bool) -> Result<Self> {
        Ok(Self {
            resampler: MonoResampler::new(rate)?,
            normalizer: LevelNormalizer::new(audio),
            converted: Vec::with_capacity(2048),
            frame: [0.0; 512],
            filled: 0,
            frame_flags: FrameFlags::EMPTY,
            next_time: start,
            mark_discontinuity: discontinuity,
        })
    }
    fn push(&mut self, block: crate::timeline::MonoBlock, output: &AudioProducer) -> Result<()> {
        self.converted.clear();
        self.resampler
            .push(&block.samples[..block.frames], &mut self.converted)?;
        // GAP_FILLED applies only if every contributing raw block is generated
        // silence. A mixed block contains real captured samples and is not a gap.
        let flags = FrameFlags {
            gap_filled: block.flags.gap_filled,
            discontinuity: false,
        };
        self.append_converted(flags, output)
    }
    fn append_converted(&mut self, flags: FrameFlags, output: &AudioProducer) -> Result<()> {
        for index in 0..self.converted.len() {
            if self.filled == 0 {
                self.frame_flags = flags;
            } else {
                self.frame_flags.gap_filled &= flags.gap_filled;
            }
            self.frame[self.filled] = self.converted[index];
            self.filled += 1;
            if self.filled == 512 {
                self.normalizer.process(&mut self.frame);
                self.frame_flags.discontinuity |= self.mark_discontinuity;
                output.send(AudioFrame {
                    t0: self.next_time,
                    samples: self.frame,
                    flags: self.frame_flags,
                })?;
                self.next_time = StreamTime(self.next_time.samples().saturating_add(512));
                self.mark_discontinuity = false;
                self.filled = 0;
            }
        }
        Ok(())
    }
    fn finish(&mut self, output: &AudioProducer) -> Result<()> {
        self.converted.clear();
        self.resampler.finish(&mut self.converted)?;
        self.append_converted(FrameFlags::EMPTY, output)?;
        // Generation boundaries keep complete prepared frames. A residual <32 ms
        // is discarded and the next epoch advances to its absolute wall position.
        self.filled = 0;
        Ok(())
    }
}

fn adapter_loop(
    worker: &mut NativeWorker,
    output: &AudioProducer,
    events: &SourceEvents,
    audio: AudioConfig,
    epoch: CaptureEpoch,
    pipeline_cancelled: &AtomicBool,
) -> Result<()> {
    let clock = QpcClock::new()?;
    let mut generation = 0;
    let mut rate = 48_000;
    let mut opened_qpc = epoch.qpc_100ns;
    let mut packet_clock = PacketClock::default();
    let mut timeline: Option<crate::timeline::MonoTimeline> = None;
    let mut prepared: Option<PreparedFrames> = None;
    let mut next_time = output.base_time;
    let mut unavailable = true;
    let mut last_state = None;
    let mut seen_overflows = 0;
    let mut silence = SilenceTracker::new(rate);
    while !worker.cancelled.load(Ordering::Acquire) && !pipeline_cancelled.load(Ordering::Acquire) {
        // Finish the previous generation's ring before processing its replacement.
        // Ready owns a new consumer, so queued old audio is never skipped by a
        // metadata update overtaking the bounded audio queue.
        let mut had_packet = false;
        while let Ok(packet) = worker.consumer.pop() {
            if packet.generation != generation {
                continue;
            }
            had_packet = true;
            let raw_t0 = packet_clock.packet_t0(&packet, rate, opened_qpc, epoch);
            if let (Some(timeline), Some(prepared)) = (&mut timeline, &mut prepared) {
                let mut emit = |block| {
                    silence.observe(&block, |state| {
                        publish_source_state(state, events, &mut last_state, unavailable)
                    });
                    prepared.push(block, output)
                };
                // DATA_DISCONTINUITY is only a gap hint: the native timestamp
                // supplies its missing interval, without resetting segmentation.
                let _gap_hint = packet.data_discontinuity;
                timeline.push_packet(
                    raw_t0,
                    &packet.samples[..usize::from(packet.frames)],
                    packet.silent,
                    &mut emit,
                )?;
            }
        }
        // Apply one notice per iteration and drain that generation before the
        // next Ready. A burst of endpoint changes still preserves audio order.
        if let Ok(notice) = worker.notices.try_recv() {
            match notice {
                NativeNotice::Ready {
                    generation: new_generation,
                    format,
                    info,
                    opened_qpc_100ns,
                    consumer,
                } => {
                    if let (Some(old_timeline), Some(old_prepared)) = (&mut timeline, &mut prepared)
                    {
                        old_timeline.flush(&mut |block| old_prepared.push(block, output))?;
                        old_prepared.finish(output)?;
                        next_time = old_prepared.next_time;
                    }
                    let start = StreamTime(
                        epoch
                            .stream_at(opened_qpc_100ns)
                            .samples()
                            .max(next_time.samples()),
                    );
                    rate = format.sample_rate();
                    opened_qpc = opened_qpc_100ns;
                    let mut new_timeline = crate::timeline::MonoTimeline::new(rate)?;
                    new_timeline.discontinuity(epoch.raw_at(opened_qpc, rate), &mut |_| Ok(()))?;
                    timeline = Some(new_timeline);
                    prepared = Some(PreparedFrames::new(
                        rate,
                        audio.clone(),
                        start,
                        generation != 0,
                    )?);
                    worker.consumer = consumer;
                    silence = SilenceTracker::new(rate);
                    packet_clock = PacketClock::default();
                    generation = new_generation;
                    unavailable = false;
                    events.publish(PipelineEvent::SourceChanged { info });
                    last_state = None;
                }
                NativeNotice::Unavailable { state, detail } => {
                    unavailable = true;
                    if last_state != Some(state) {
                        events.publish(PipelineEvent::SourceState {
                            state,
                            detail: Some(detail),
                        });
                        last_state = Some(state);
                    }
                }
            }
        }
        if worker.consumer.slots() == 0 && worker.notices.is_empty() {
            if let (Some(timeline), Some(prepared)) = (&mut timeline, &mut prepared) {
                let watermark = if unavailable {
                    clock.now()?.saturating_sub(500_000)
                } else {
                    worker.timeout_qpc_100ns.load(Ordering::Acquire)
                };
                let mut emit = |block| {
                    silence.observe(&block, |state| {
                        publish_source_state(state, events, &mut last_state, unavailable)
                    });
                    prepared.push(block, output)
                };
                timeline.timeout_until(epoch.raw_at(watermark, rate), &mut emit)?;
            }
        }
        let overflows = worker.overflows.load(Ordering::Relaxed);
        if overflows != seen_overflows {
            tracing::warn!(
                lost_source_frames = overflows - seen_overflows,
                "WASAPI ring overflow; timeline fills the lost span"
            );
            seen_overflows = overflows;
        }
        if worker.join.as_ref().is_some_and(JoinHandle::is_finished)
            && worker.notices.is_empty()
            && worker.consumer.slots() == 0
            && !had_packet
        {
            return Ok(());
        }
        if !had_packet {
            thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}

fn publish_source_state(
    state: SourceStateKind,
    events: &SourceEvents,
    last_state: &mut Option<SourceStateKind>,
    unavailable: bool,
) {
    if unavailable {
        return;
    }
    if *last_state != Some(state) {
        events.publish(PipelineEvent::SourceState {
            state,
            detail: None,
        });
        *last_state = Some(state);
    }
}

struct SilenceTracker {
    sample_rate: u32,
    zeros: u64,
    state: Option<SourceStateKind>,
}
impl SilenceTracker {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            zeros: 0,
            state: None,
        }
    }
    fn observe(
        &mut self,
        block: &crate::timeline::MonoBlock,
        mut changed: impl FnMut(SourceStateKind),
    ) {
        for sample in &block.samples[..block.frames] {
            if *sample == 0.0 {
                self.zeros = self.zeros.saturating_add(1);
            } else {
                self.zeros = 0;
            }
            let state = if self.zeros >= u64::from(self.sample_rate) * 3 {
                SourceStateKind::Silent
            } else {
                SourceStateKind::Playing
            };
            if self.state != Some(state) {
                self.state = Some(state);
                changed(state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::{bus::EventBus, types::CaptureMode};

    fn format(tag: u16, bits: u16, channels: u16, rate: u32) -> CaptureFormat {
        let alignment = channels * (bits / 8);
        CaptureFormat::from_wave(
            WAVEFORMATEX {
                wFormatTag: tag,
                nChannels: channels,
                nSamplesPerSec: rate,
                nAvgBytesPerSec: rate * u32::from(alignment),
                nBlockAlign: alignment,
                wBitsPerSample: bits,
                cbSize: 0,
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn extensible_formats_validate_mask_and_valid_bits_before_capture() {
        let mut wave = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: 0xfffe,
                nChannels: 6,
                nSamplesPerSec: 48_000,
                nAvgBytesPerSec: 1_152_000,
                nBlockAlign: 24,
                wBitsPerSample: 32,
                cbSize: 22,
            },
            Samples: windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: 24,
            },
            dwChannelMask: 0x3f,
            SubFormat: PCM_GUID,
        };
        let format = CaptureFormat::from_wave(wave.Format, Some(wave)).unwrap();
        assert_eq!(format.identity.valid_bits, 24);
        assert_eq!(format.weights[2], std::f32::consts::FRAC_1_SQRT_2);
        assert_eq!(format.weights[3], 0.0);
        wave.dwChannelMask = 3;
        assert!(CaptureFormat::from_wave(wave.Format, Some(wave)).is_err());
        wave.dwChannelMask = 0x3f;
        wave.Samples.wValidBitsPerSample = 33;
        assert!(CaptureFormat::from_wave(wave.Format, Some(wave)).is_err());
        assert!(unsafe { CaptureFormat::copy_mix_format(std::ptr::null()) }.is_err());
    }

    #[test]
    fn silence_requires_exact_three_seconds_and_real_audio_resumes_playing() {
        let mut silence = SilenceTracker::new(16_000);
        let mut changes = Vec::new();
        let mut block = crate::timeline::MonoBlock {
            t0: 0,
            sample_rate: 16_000,
            frames: 512,
            samples: [0.0; 512],
            flags: FrameFlags::GAP_FILLED,
        };
        for _ in 0..93 {
            silence.observe(&block, |state| changes.push(state));
        }
        block.frames = 383;
        silence.observe(&block, |state| changes.push(state));
        assert_eq!(changes, [SourceStateKind::Playing]);
        block.frames = 1;
        silence.observe(&block, |state| changes.push(state));
        assert_eq!(changes, [SourceStateKind::Playing, SourceStateKind::Silent]);
        block.samples[0] = 0.01;
        block.flags = FrameFlags::EMPTY;
        silence.observe(&block, |state| changes.push(state));
        assert_eq!(changes.last(), Some(&SourceStateKind::Playing));
    }

    #[test]
    fn fake_rate_change_drains_old_ring_rebuilds_dsp_and_marks_one_boundary() {
        let epoch = 1_000_000;
        let (_, empty) = RingBuffer::new(1);
        let (sender, notices) = crossbeam_channel::bounded(4);
        for (generation, rate, count, start_qpc) in
            [(1, 48_000, 12, epoch), (2, 16_000, 8, epoch + 10_000_000)]
        {
            let (mut producer, consumer) = RingBuffer::new(count);
            for index in 0..count {
                let offset = index as u64 * 512;
                producer
                    .push(CapturePacket {
                        generation,
                        frames: 512,
                        samples: [0.1; 512],
                        device_position: offset,
                        qpc_100ns: start_qpc + samples_to_hns(offset, rate),
                        silent: false,
                        ..Default::default()
                    })
                    .ok()
                    .unwrap();
            }
            sender
                .send(NativeNotice::Ready {
                    generation,
                    format: format(3, 32, 1, rate),
                    info: SourceInfo {
                        mode: CaptureMode::System,
                        label: format!("Device {generation}"),
                        sample_rate: rate,
                        channels: 1,
                    },
                    opened_qpc_100ns: start_qpc,
                    consumer,
                })
                .ok()
                .unwrap();
        }
        drop(sender);
        let worker = NativeWorker {
            consumer: empty,
            notices,
            overflows: Arc::new(AtomicU64::new(0)),
            cancelled: Arc::new(AtomicBool::new(false)),
            join: Some(thread::spawn(|| Ok(()))),
            start_qpc_100ns: epoch,
            timeout_qpc_100ns: Arc::new(AtomicU64::new(epoch)),
        };
        let (sender, receiver) = crossbeam_channel::bounded(32);
        let output = AudioProducer::new(sender, Arc::new(AtomicBool::new(false)), StreamTime::ZERO);
        let bus = EventBus::default();
        let events = bus.subscribe(16);
        run_adapter(
            worker,
            output,
            SourceEvents::new(bus),
            AudioConfig {
                normalize: false,
                ..Default::default()
            },
        )
        .unwrap();
        let frames: Vec<_> = receiver.try_iter().collect();
        assert_eq!(frames.len(), 12);
        assert_eq!(frames[0].t0, StreamTime::ZERO);
        assert_eq!(frames[3].t0, StreamTime(1536));
        assert_eq!(frames[4].t0, StreamTime(16_000));
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.flags.discontinuity)
                .count(),
            1
        );
        assert!(frames[4].flags.discontinuity);
        assert_eq!(
            events
                .try_iter()
                .filter_map(|event| match event {
                    PipelineEvent::SourceChanged { info } => Some(info.sample_rate),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [48_000, 16_000]
        );
    }
}
