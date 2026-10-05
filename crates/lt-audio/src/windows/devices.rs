//! Endpoint discovery and notification lifetimes. Never called from a packet callback.
use lt_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
};
use windows::{
    core::{implement, PCWSTR},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::PROPERTYKEY,
        Media::Audio::{
            eConsole, eRender, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator,
            IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
            DEVICE_STATE_ACTIVE,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
            StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
            CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
        },
    },
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DeviceChoice {
    #[default]
    FollowDefault,
    Id(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub(crate) fn native_error(error: windows::core::Error) -> Error {
    Error::Engine(format!("Windows audio: {error}"))
}

/// Declare this before COM interfaces so it is dropped after those interfaces.
pub(crate) struct MtaGuard;
impl MtaGuard {
    pub(crate) fn new() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(native_error)?;
        Ok(Self)
    }
}
impl Drop for MtaGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(crate) fn create_enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(native_error)
}

pub(crate) fn choose_device(
    enumerator: &IMMDeviceEnumerator,
    choice: &DeviceChoice,
) -> Result<IMMDevice> {
    match choice {
        DeviceChoice::FollowDefault => {
            unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }.map_err(native_error)
        }
        DeviceChoice::Id(id) => {
            if id.is_empty() || id.contains('\0') {
                return Err(Error::Config(
                    "Audio endpoint ID is empty or contains NUL".into(),
                ));
            }
            let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
            unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr())) }.map_err(native_error)
        }
    }
}

pub(crate) fn device_id(device: &IMMDevice) -> Result<String> {
    let id = unsafe { device.GetId() }.map_err(native_error)?;
    let result = unsafe { id.to_string() }
        .map_err(|error| Error::Engine(format!("Invalid endpoint ID: {error}")));
    unsafe { CoTaskMemFree(Some(id.0.cast())) };
    result
}

pub(crate) fn device_name(device: &IMMDevice) -> Result<String> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.map_err(native_error)?;
    let mut value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.map_err(native_error)?;
    let converted = unsafe { PropVariantToStringAlloc(&value) }.map_err(native_error);
    let result = match converted {
        Ok(text) => {
            let result = unsafe { text.to_string() }
                .map_err(|error| Error::Engine(format!("Invalid endpoint name: {error}")));
            unsafe { CoTaskMemFree(Some(text.0.cast())) };
            result
        }
        Err(error) => Err(error),
    };
    // PROPVARIANT and returned string are independently owned allocations.
    unsafe { PropVariantClear(&mut value) }.map_err(native_error)?;
    result
}

/// Read-only discovery in its own MTA; callers' apartment settings are untouched.
pub fn list_audio_devices() -> Result<Vec<AudioDevice>> {
    thread::Builder::new()
        .name("lt-device-list".into())
        .spawn(|| {
            let _mta = MtaGuard::new()?;
            let enumerator = create_enumerator()?;
            let default = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                .ok()
                .and_then(|device| device_id(&device).ok());
            let collection = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
                .map_err(native_error)?;
            let count = unsafe { collection.GetCount() }.map_err(native_error)?;
            let mut devices = Vec::with_capacity(count as usize);
            for index in 0..count {
                let device = unsafe { collection.Item(index) }.map_err(native_error)?;
                let id = device_id(&device)?;
                let name = device_name(&device).unwrap_or_else(|_| id.clone());
                devices.push(AudioDevice {
                    is_default: default.as_ref() == Some(&id),
                    id,
                    name,
                });
            }
            Ok(devices)
        })?
        .join()
        .map_err(|_| Error::Engine("Audio device discovery worker panicked".into()))?
}

/// The callback only advances a counter: no strings, logs, locks or COM queries.
#[implement(IMMNotificationClient)]
struct EndpointNotification {
    dirty: Arc<AtomicU64>,
}

impl IMMNotificationClient_Impl for EndpointNotification_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        self.dirty.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.dirty.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.dirty.fetch_add(1, Ordering::Release);
        Ok(())
    }
    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            self.dirty.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
        self.dirty.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

pub(crate) struct NotificationGuard {
    enumerator: IMMDeviceEnumerator,
    callback: IMMNotificationClient,
}
impl NotificationGuard {
    pub(crate) fn new(enumerator: &IMMDeviceEnumerator, dirty: Arc<AtomicU64>) -> Result<Self> {
        let callback: IMMNotificationClient = EndpointNotification { dirty }.into();
        unsafe { enumerator.RegisterEndpointNotificationCallback(&callback) }
            .map_err(native_error)?;
        Ok(Self {
            enumerator: enumerator.clone(),
            callback,
        })
    }
}
impl Drop for NotificationGuard {
    fn drop(&mut self) {
        // Unregister outside the callback, before dropping its retained interfaces.
        let _ = unsafe {
            self.enumerator
                .UnregisterEndpointNotificationCallback(&self.callback)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notifications_only_signal_render_console_and_do_not_dereference_ids() {
        let dirty = Arc::new(AtomicU64::new(0));
        let notification: IMMNotificationClient = EndpointNotification {
            dirty: dirty.clone(),
        }
        .into();
        unsafe {
            notification.OnDeviceAdded(PCWSTR::null()).unwrap();
            notification
                .OnDefaultDeviceChanged(
                    windows::Win32::Media::Audio::eCapture,
                    eConsole,
                    PCWSTR::null(),
                )
                .unwrap();
            notification
                .OnDefaultDeviceChanged(eRender, eConsole, PCWSTR::null())
                .unwrap();
        }
        assert_eq!(dirty.load(Ordering::Acquire), 2);
    }
}
