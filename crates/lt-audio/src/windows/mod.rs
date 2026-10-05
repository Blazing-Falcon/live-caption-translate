//! Windows capture sources. Native COM and device operations start only on workers.
pub(crate) mod capture;
pub mod devices;

pub use devices::{list_audio_devices, AudioDevice, DeviceChoice};

use capture::{CaptureFormat, NativeWorker, OpenedClient, WorkerOptions};
use devices::{choose_device, device_id, device_name, native_error};
use lt_core::{
    config::AudioConfig,
    error::{Error, Result},
    events::{PipelineEvent, SourceStateKind},
    source::{AudioProducer, AudioSource, SourceEvents},
    types::{CaptureMode, SourceInfo},
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};
use windows::Win32::{
    Media::Audio::{IAudioClient, IMMDeviceEnumerator},
    System::Com::{CoTaskMemFree, CLSCTX_ALL},
};

fn open_endpoint(
    enumerator: &IMMDeviceEnumerator,
    choice: &DeviceChoice,
    cancelled: &AtomicBool,
) -> Result<OpenedClient> {
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Stopped);
    }
    let device = choose_device(enumerator, choice)?;
    let identity = device_id(&device)?;
    let label = device_name(&device).unwrap_or_else(|_| identity.clone());
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(native_error)?;
    let allocated = unsafe { client.GetMixFormat() }.map_err(native_error)?;
    let format = unsafe { CaptureFormat::copy_mix_format(allocated) };
    unsafe { CoTaskMemFree(Some(allocated.cast())) };
    let format = format?;
    let info = SourceInfo {
        mode: CaptureMode::System,
        label,
        sample_rate: format.sample_rate(),
        channels: format.channels(),
    };
    Ok(OpenedClient {
        client,
        format,
        info,
        identity,
        autoconvert: false,
        buffer_duration_hns: 1_000_000,
    })
}

/// System loopback follows the chosen render endpoint. Construction is pure;
/// start owns one MTA/MMCSS capture worker and one prepared-frame adapter.
pub struct LoopbackSource {
    choice: DeviceChoice,
    audio: AudioConfig,
    cancelled: Arc<AtomicBool>,
    adapter: Option<JoinHandle<Result<()>>>,
}
impl LoopbackSource {
    pub fn new(choice: DeviceChoice, audio: AudioConfig) -> Result<Self> {
        if matches!(&choice, DeviceChoice::Id(id) if id.is_empty() || id.contains('\0')) {
            return Err(Error::Config(
                "Audio endpoint ID is empty or contains NUL".into(),
            ));
        }
        Ok(Self {
            choice,
            audio,
            cancelled: Arc::new(AtomicBool::new(false)),
            adapter: None,
        })
    }
    fn pending_info(&self) -> SourceInfo {
        SourceInfo {
            mode: CaptureMode::System,
            label: match &self.choice {
                DeviceChoice::FollowDefault => "Default output".into(),
                DeviceChoice::Id(_) => "Selected output".into(),
            },
            sample_rate: 48_000,
            channels: 2,
        }
    }
}
impl AudioSource for LoopbackSource {
    fn start(&mut self, output: AudioProducer, events: SourceEvents) -> Result<SourceInfo> {
        if self.adapter.is_some() {
            return Err(Error::Engine(
                "System loopback source is already running".into(),
            ));
        }
        self.cancelled = Arc::new(AtomicBool::new(false));
        let choice = self.choice.clone();
        let worker = NativeWorker::spawn(
            Box::new(move |enumerator, cancelled| open_endpoint(enumerator, &choice, cancelled)),
            WorkerOptions {
                watch_devices: true,
                unavailable_state: SourceStateKind::NoDevice,
            },
            self.cancelled.clone(),
        )?;
        let audio = self.audio.clone();
        self.adapter = Some(
            thread::Builder::new()
                .name("lt-system-adapter".into())
                .spawn(move || {
                    let result = capture::run_adapter(worker, output, events.clone(), audio);
                    if let Err(error) = &result {
                        events.publish(PipelineEvent::SourceState {
                            state: SourceStateKind::NoDevice,
                            detail: Some(error.to_string()),
                        });
                    }
                    result
                })?,
        );
        // A missing endpoint is a recoverable state; background reopen retains
        // the pipeline's loaded engines. Actual format arrives in SourceChanged.
        Ok(self.pending_info())
    }
    fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(adapter) = self.adapter.take() {
            let _ = adapter.join();
        }
    }
}
impl Drop for LoopbackSource {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_endpoint_ids_fail_before_any_native_operation() {
        assert!(matches!(
            LoopbackSource::new(DeviceChoice::Id(String::new()), AudioConfig::default()),
            Err(Error::Config(_))
        ));
        assert!(matches!(
            LoopbackSource::new(DeviceChoice::Id("a\0b".into()), AudioConfig::default()),
            Err(Error::Config(_))
        ));
        let source =
            LoopbackSource::new(DeviceChoice::FollowDefault, AudioConfig::default()).unwrap();
        assert_eq!(source.pending_info().mode, CaptureMode::System);
    }
}
