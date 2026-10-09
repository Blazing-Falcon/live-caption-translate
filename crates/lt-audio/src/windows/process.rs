//! Cancellable native process-loopback activation with retained COM parameters.
use super::{
    apps::{process_loopback_supported, windows_build},
    capture::{CaptureFormat, NativeWorker, OpenedClient, WorkerOptions},
};
use lt_core::{
    config::AudioConfig,
    error::{Error, Result},
    events::{PipelineEvent, SourceStateKind},
    source::{AudioProducer, AudioSource, SourceEvents},
    types::{CaptureMode, SourceInfo},
};
use std::{
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    core::{implement, IUnknown, Interface, Ref, HRESULT},
    Win32::{
        Media::Audio::{
            ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
            IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioClient,
            AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
            AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        },
        System::{
            Com::{
                StructuredStorage::{
                    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
                },
                BLOB,
            },
            Variant::VT_BLOB,
        },
    },
};

const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(10);
const ACTIVATION_POLL: Duration = Duration::from_millis(20);

/// Immutable pinned allocations are held by the completion handler as well as
/// the waiter. Cancellation cannot invalidate native async parameter pointers.
struct ActivationPayload {
    _params: Box<AUDIOCLIENT_ACTIVATION_PARAMS>,
    variant: ManuallyDrop<PROPVARIANT>,
}

impl ActivationPayload {
    fn new(root_pid: u32) -> Self {
        let mut params = Box::new(AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: root_pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        });
        let pointer = std::ptr::from_mut(params.as_mut()).cast();
        let variant = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: pointer,
                        },
                    },
                }),
            },
        };
        Self {
            _params: params,
            variant: ManuallyDrop::new(variant),
        }
    }
}

// SAFETY: The sole pointer refers to the payload's stable Box allocation. Both
// structures are immutable after creation, and Windows reads the VT_BLOB; it
// never owns/frees its Rust allocation. The callback retains the complete Arc.
unsafe impl Send for ActivationPayload {}
unsafe impl Sync for ActivationPayload {}

// windows-implement implements IAgileObject and the free-threaded marshaler by
// default. Keep that default: ActivateCompleted may arrive on another MTA thread.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler {
    completed: Arc<AtomicBool>,
    _payload: Arc<ActivationPayload>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        self.completed.store(true, Ordering::Release);
        Ok(())
    }
}

fn wait_completed(completed: &AtomicBool, cancelled: &AtomicBool, deadline: Instant) -> Result<()> {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        if completed.load(Ordering::Acquire) {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Engine(
                "Timed out activating selected-app loopback capture".into(),
            ));
        }
        thread::sleep(remaining.min(ACTIVATION_POLL));
    }
}

/// Caller is the shared capture worker in an initialized MTA apartment. No
/// interface is sent across apartments; the final result is retrieved here.
pub(crate) fn activate_process_client(
    root_pid: u32,
    cancelled: &AtomicBool,
) -> Result<IAudioClient> {
    if root_pid == 0 {
        return Err(Error::Config(
            "Selected-app process ID cannot be zero".into(),
        ));
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Stopped);
    }
    let payload = Arc::new(ActivationPayload::new(root_pid));
    let completed = Arc::new(AtomicBool::new(false));
    let handler: IActivateAudioInterfaceCompletionHandler = ActivationHandler {
        completed: completed.clone(),
        _payload: payload.clone(),
    }
    .into();
    // Native activation retains the handler until completion. That retained
    // reference owns both the PROPVARIANT and the blob through cancellation.
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(std::ptr::from_ref(&*payload.variant)),
            &handler,
        )
    }
    .map_err(activation_error)?;
    wait_completed(&completed, cancelled, Instant::now() + ACTIVATION_TIMEOUT)?;
    let mut result = HRESULT::default();
    let mut client: Option<IUnknown> = None;
    unsafe { operation.GetActivateResult(&mut result, &mut client) }.map_err(activation_error)?;
    result.ok().map_err(activation_error)?;
    client
        .ok_or_else(|| {
            Error::Engine("Process-loopback activation completed without an audio client".into())
        })?
        .cast()
        .map_err(activation_error)
}

fn activation_error(error: windows::core::Error) -> Error {
    Error::Engine(format!("Activating selected-app loopback capture: {error}"))
}

pub(crate) fn open_process(root_pid: u32, cancelled: &AtomicBool) -> Result<OpenedClient> {
    let client = activate_process_client(root_pid, cancelled)?;
    Ok(OpenedClient {
        client,
        format: CaptureFormat::float_stereo_48k(),
        info: process_info(root_pid),
        identity: format!("process:{root_pid}"),
        autoconvert: true,
        buffer_duration_hns: 0,
    })
}

fn process_info(root_pid: u32) -> SourceInfo {
    SourceInfo {
        mode: CaptureMode::Apps,
        label: format!("Process {root_pid}"),
        sample_rate: 48_000,
        channels: 2,
    }
}

/// One fixed-48k float-stereo stream including the target process's children.
/// Session selection/restart tracking belongs to the apps capture manager.
pub struct ProcessLoopbackSource {
    root_pid: u32,
    audio: AudioConfig,
    cancelled: Arc<AtomicBool>,
    adapter: Option<JoinHandle<Result<()>>>,
}

impl ProcessLoopbackSource {
    /// Capability validation only; activation starts in AudioSource::start.
    pub fn new(root_pid: u32, audio: AudioConfig) -> Result<Self> {
        if root_pid == 0 {
            return Err(Error::Config(
                "Selected-app process ID cannot be zero".into(),
            ));
        }
        if !process_loopback_supported(windows_build()?) {
            return Err(Error::Engine(
                "Selected apps requires Windows build 20348 or later".into(),
            ));
        }
        Ok(Self {
            root_pid,
            audio,
            cancelled: Arc::new(AtomicBool::new(false)),
            adapter: None,
        })
    }
}

impl AudioSource for ProcessLoopbackSource {
    fn start(&mut self, output: AudioProducer, events: SourceEvents) -> Result<SourceInfo> {
        if self.adapter.is_some() {
            return Err(Error::Engine(
                "Process loopback source is already running".into(),
            ));
        }
        self.cancelled = Arc::new(AtomicBool::new(false));
        let root_pid = self.root_pid;
        let worker = NativeWorker::spawn(
            Box::new(move |_, cancelled| open_process(root_pid, cancelled)),
            WorkerOptions {
                watch_devices: false,
                unavailable_state: SourceStateKind::AppsNotRunning,
            },
            self.cancelled.clone(),
        )?;
        let audio = self.audio.clone();
        self.adapter = Some(
            thread::Builder::new()
                .name(format!("lt-process-adapter-{root_pid}"))
                .spawn(move || {
                    let result = super::capture::run_adapter(worker, output, events.clone(), audio);
                    if let Err(error) = &result {
                        events.publish(PipelineEvent::SourceState {
                            state: SourceStateKind::AppsNotRunning,
                            detail: Some(error.to_string()),
                        });
                    }
                    result
                })?,
        );
        Ok(process_info(root_pid))
    }

    fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(adapter) = self.adapter.take() {
            let _ = adapter.join();
        }
    }
}

impl Drop for ProcessLoopbackSource {
    fn drop(&mut self) {
        self.stop();
    }
}
