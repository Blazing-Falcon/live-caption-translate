//! Portable-zip WebView2 preflight. Tauri's `webviewInstallMode` only applies to installers, so a
//! bare executable checks for the runtime itself and runs the bundled bootstrapper when missing.
use std::{os::windows::process::CommandExt, path::PathBuf, process::Command};
use webview2_com::Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString;
use windows::{
    core::{w, PCWSTR, PWSTR},
    Win32::{
        System::Com::CoTaskMemFree,
        UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
    },
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const BOOTSTRAPPER: &str = "MicrosoftEdgeWebview2Setup.exe";

pub fn installed_version() -> Option<String> {
    let mut version = PWSTR::null();
    let result =
        unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) };
    if result.is_err() || version.is_null() {
        return None;
    }
    let text = unsafe { version.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(version.0.cast())) };
    text.filter(|text| !text.is_empty())
}

fn bootstrapper() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join(BOOTSTRAPPER);
    path.is_file().then_some(path)
}

/// Returns true when a WebView2 runtime is available after the check, installing if needed.
pub fn ensure_runtime() -> bool {
    if let Some(version) = installed_version() {
        tracing::info!(version, "WebView2 runtime found");
        return true;
    }
    if let Some(setup) = bootstrapper() {
        tracing::warn!("WebView2 runtime missing; running the bundled bootstrapper");
        let status = Command::new(setup)
            .args(["/silent", "/install"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => tracing::warn!(%status, "WebView2 bootstrapper reported a failure"),
            Err(error) => tracing::warn!(%error, "WebView2 bootstrapper could not run"),
        }
        if installed_version().is_some() {
            return true;
        }
    }
    unsafe {
        MessageBoxW(
            None,
            w!("Live Translation needs the Microsoft Edge WebView2 Runtime, which could not be installed automatically.\n\nInstall it from https://go.microsoft.com/fwlink/p/?LinkId=2124703 and start the app again."),
            w!("Live Translation"),
            MB_OK | MB_ICONERROR,
        );
    }
    false
}
