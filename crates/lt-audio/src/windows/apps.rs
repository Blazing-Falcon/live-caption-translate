//! Audio-session metadata and executable-based selections. No capture starts here.
use base64::{engine::general_purpose::STANDARD, Engine};
use lt_core::{
    config::CaptureApp,
    error::{Error, Result},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_NO_MORE_FILES, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
    },
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Threading::{
            GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

pub const MIN_PROCESS_LOOPBACK_BUILD: u32 = 20_348;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioApp {
    pub exe: String,
    pub name: String,
    pub pid: u32,
    pub icon_png: Option<String>,
    pub peak: f32,
    pub active: bool,
    pub recent: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioAppList {
    pub supported: bool,
    pub reason: Option<String>,
    pub apps: Vec<AudioApp>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub exe: String,
    pub path: Option<PathBuf>,
    /// Prevents an older child from adopting an unrelated reused parent PID.
    pub created_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSession {
    pub pid: u32,
    pub root_pid: u32,
    pub exe: String,
    pub name: String,
    pub icon_png: Option<String>,
    pub peak: f32,
    pub active: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub processes: Vec<ProcessInfo>,
    pub sessions: Vec<AudioSession>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppTarget {
    pub root_pid: u32,
    pub exe: String,
    pub name: String,
    pub created_at: Option<u64>,
}

pub fn process_loopback_supported(build: u32) -> bool {
    build >= MIN_PROCESS_LOOPBACK_BUILD
}

pub fn windows_build() -> Result<u32> {
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(version: *mut OSVERSIONINFOW) -> i32;
    }
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..OSVERSIONINFOW::default()
    };
    // RtlGetVersion is independent of executable compatibility manifests.
    if unsafe { RtlGetVersion(&mut version) } < 0 {
        return Err(Error::Engine("Cannot read the Windows build number".into()));
    }
    Ok(version.dwBuildNumber)
}

/// Runs COM discovery on its own MTA thread and returns the exact picker DTO.
/// Call only in response to an application/user request; tests use pure inputs.
pub fn list_audio_apps(recent: &[CaptureApp]) -> Result<AudioAppList> {
    let build = windows_build()?;
    if !process_loopback_supported(build) {
        return Ok(app_list(build, &[], recent));
    }
    let snapshot = enumerate_sessions()?;
    Ok(app_list(build, &snapshot.sessions, recent))
}

pub fn enumerate_sessions() -> Result<SessionSnapshot> {
    thread::Builder::new()
        .name("lt-audio-app-discovery".into())
        .spawn(|| {
            let _com = DiscoveryMta::new()?;
            enumerate_on_worker()
        })?
        .join()
        .map_err(|_| Error::Engine("Audio-session discovery worker stopped unexpectedly".into()))?
}

pub(crate) struct DiscoveryMta;
impl DiscoveryMta {
    pub(crate) fn new() -> Result<Self> {
        wasapi::initialize_mta()
            .ok()
            .map_err(|error| Error::Engine(format!("Initializing audio-session COM: {error}")))?;
        Ok(Self)
    }
}
impl Drop for DiscoveryMta {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}

/// Caller owns an MTA apartment. COM handles never leave this worker.
pub(crate) fn enumerate_on_worker() -> Result<SessionSnapshot> {
    enumerate_on_worker_cancellable(&AtomicBool::new(false))
}

pub(crate) fn enumerate_on_worker_cancellable(cancelled: &AtomicBool) -> Result<SessionSnapshot> {
    scan_sessions(cancelled, true)
}

/// Capture selection needs process/session identity, not repeated PNG extraction.
pub(crate) fn enumerate_capture_sessions(cancelled: &AtomicBool) -> Result<SessionSnapshot> {
    scan_sessions(cancelled, false)
}

fn scan_sessions(cancelled: &AtomicBool, rich_metadata: bool) -> Result<SessionSnapshot> {
    check_cancel(cancelled)?;
    let processes = process_snapshot_cancellable(cancelled, rich_metadata)?;
    let by_pid: BTreeMap<_, _> = processes
        .iter()
        .map(|process| (process.pid, process))
        .collect();
    let enumerator = wasapi::DeviceEnumerator::new().map_err(discovery_error)?;
    let devices = enumerator
        .get_device_collection(&wasapi::Direction::Render)
        .map_err(discovery_error)?;
    let mut sessions = Vec::new();
    let mut seen = BTreeSet::new();
    for device in &devices {
        check_cancel(cancelled)?;
        let Ok(device) = device else {
            continue;
        };
        let Ok(manager) = device.get_iaudiosessionmanager() else {
            continue;
        };
        let Ok(collection) = manager.get_audiosessionenumerator() else {
            continue;
        };
        // GetCount also initializes the session-enumeration notification state.
        let count = collection.get_count().map_err(discovery_error)?;
        for index in 0..count {
            check_cancel(cancelled)?;
            let Ok(session) = collection.get_session(index) else {
                continue;
            };
            let Ok(state) = session.get_state() else {
                continue;
            };
            if state == wasapi::SessionState::Expired {
                continue;
            }
            let Ok(pid) = session.get_process_id() else {
                continue;
            };
            let Some(process) = by_pid.get(&pid).filter(|_| pid != 0) else {
                continue;
            };
            let identifier = session
                .get_session_instance_identifier()
                .unwrap_or_default();
            if !identifier.is_empty() && !seen.insert(identifier) {
                continue;
            }
            let root_pid = root_from_snapshot(pid, &by_pid);
            let root = by_pid.get(&root_pid).copied().unwrap_or(process);
            let name = session.get_display_name().unwrap_or_default();
            let name = session_display_name(&name, &root.exe);
            let peak = if rich_metadata {
                session
                    .get_audiometerinformation()
                    .and_then(|meter| meter.get_peak_value())
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let icon_png = if rich_metadata {
                let icon_path = session.get_icon_path().unwrap_or_default();
                session_icon(&icon_path, root.path.as_deref())
            } else {
                None
            };
            sessions.push(AudioSession {
                pid,
                root_pid,
                exe: root.exe.clone(),
                name,
                icon_png,
                peak: finite_peak(peak),
                active: state == wasapi::SessionState::Active,
            });
        }
    }
    Ok(SessionSnapshot {
        processes,
        sessions,
    })
}

fn discovery_error(error: impl std::fmt::Display) -> Error {
    Error::Engine(format!("Enumerating audio sessions: {error}"))
}

struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub fn process_snapshot() -> Result<Vec<ProcessInfo>> {
    process_snapshot_cancellable(&AtomicBool::new(false), true)
}

fn check_cancel(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(Error::Stopped)
    } else {
        Ok(())
    }
}

fn process_snapshot_cancellable(
    cancelled: &AtomicBool,
    query_paths: bool,
) -> Result<Vec<ProcessInfo>> {
    check_cancel(cancelled)?;
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    let snapshot = ProcessHandle(raw);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    let mut processes = Vec::new();
    let mut available = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while available {
        check_cancel(cancelled)?;
        if entry.th32ProcessID != 0 {
            let exe = utf16_z(&entry.szExeFile);
            let (path, created_at) = process_metadata(entry.th32ProcessID, query_paths);
            processes.push(ProcessInfo {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
                exe,
                path,
                created_at,
            });
        }
        available = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    let error = unsafe { GetLastError() };
    if error != ERROR_NO_MORE_FILES {
        return Err(Error::Io(std::io::Error::from_raw_os_error(error as i32)));
    }
    Ok(processes)
}

fn process_metadata(pid: u32, query_path: bool) -> (Option<PathBuf>, Option<u64>) {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return (None, None);
    }
    let handle = ProcessHandle(handle);
    let path = if query_path {
        let mut path = vec![0_u16; 32_768];
        let mut length = path.len() as u32;
        (unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) } != 0)
            .then(|| PathBuf::from(String::from_utf16_lossy(&path[..length as usize])))
    } else {
        None
    };
    let mut created = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let created_at =
        (unsafe { GetProcessTimes(handle.0, &mut created, &mut exit, &mut kernel, &mut user) }
            != 0)
            .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime));
    (path, created_at)
}

fn utf16_z(text: &[u16]) -> String {
    String::from_utf16_lossy(
        &text[..text
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(text.len())],
    )
}
fn display_name(exe: &str) -> String {
    exe.strip_suffix(".exe")
        .or_else(|| exe.strip_suffix(".EXE"))
        .unwrap_or(exe)
        .to_owned()
}

fn session_display_name(name: &str, exe: &str) -> String {
    if name.trim().is_empty() {
        return display_name(exe);
    }
    if name.starts_with('@') {
        if icon_location(name).is_none() {
            return display_name(exe);
        }
        use windows::{core::PCWSTR, Win32::UI::Shell::SHLoadIndirectString};
        let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        let mut text = [0_u16; 2048];
        if unsafe { SHLoadIndirectString(PCWSTR(wide.as_ptr()), &mut text, None) }.is_ok() {
            return utf16_z(&text);
        }
        return display_name(exe);
    }
    name.to_owned()
}
fn finite_peak(peak: f32) -> f32 {
    if peak.is_finite() {
        peak.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Topmost same-executable ancestor, with cycle and PID-reuse protection.
pub fn root_process(pid: u32, processes: &[ProcessInfo]) -> u32 {
    let by_pid: BTreeMap<_, _> = processes
        .iter()
        .map(|process| (process.pid, process))
        .collect();
    root_from_snapshot(pid, &by_pid)
}

fn root_from_snapshot(pid: u32, by_pid: &BTreeMap<u32, &ProcessInfo>) -> u32 {
    let Some(original) = by_pid.get(&pid) else {
        return pid;
    };
    let mut root = pid;
    let mut current = *original;
    let mut visited = BTreeSet::from([pid]);
    while let Some(parent) = by_pid.get(&current.parent_pid).copied() {
        if !visited.insert(parent.pid)
            || matches!((parent.created_at, current.created_at), (Some(parent), Some(child)) if parent > child)
        {
            break;
        }
        if parent.exe.eq_ignore_ascii_case(&original.exe) {
            root = parent.pid;
        }
        current = parent;
    }
    root
}

pub fn app_list(build: u32, sessions: &[AudioSession], recent: &[CaptureApp]) -> AudioAppList {
    let supported = process_loopback_supported(build);
    let mut apps = BTreeMap::<String, AudioApp>::new();
    if supported {
        for session in sessions {
            let key = session.exe.to_ascii_lowercase();
            let row = apps.entry(key).or_insert_with(|| AudioApp {
                exe: session.exe.clone(),
                name: session.name.clone(),
                pid: session.root_pid,
                icon_png: session.icon_png.clone(),
                peak: 0.0,
                active: false,
                recent: false,
            });
            row.peak = row.peak.max(finite_peak(session.peak));
            row.active |= session.active;
            row.pid = row.pid.min(session.root_pid);
            if row.icon_png.is_none() {
                row.icon_png = session.icon_png.clone();
            }
        }
    }
    for selection in recent.iter().filter(|app| !app.exe.is_empty()) {
        let row = apps
            .entry(selection.exe.to_ascii_lowercase())
            .or_insert_with(|| AudioApp {
                exe: selection.exe.clone(),
                name: if selection.name.is_empty() {
                    display_name(&selection.exe)
                } else {
                    selection.name.clone()
                },
                pid: 0,
                icon_png: None,
                peak: 0.0,
                active: false,
                recent: true,
            });
        row.recent = true;
    }
    let mut apps: Vec<_> = apps.into_values().collect();
    apps.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.exe.cmp(&right.exe))
    });
    AudioAppList { supported, reason: (!supported).then(|| "Selected apps requires Windows build 20348 or later. Whole system capture is available.".into()), apps }
}

/// Resolve every independent root of selected executables. Never capture an
/// already included ancestor's subtree twice, even across different app names.
pub fn selected_targets(selected: &[CaptureApp], snapshot: &SessionSnapshot) -> Vec<AppTarget> {
    let selected: BTreeMap<_, _> = selected
        .iter()
        .map(|app| (app.exe.to_ascii_lowercase(), app))
        .collect();
    let processes: BTreeMap<_, _> = snapshot
        .processes
        .iter()
        .map(|process| (process.pid, process))
        .collect();
    let mut targets = BTreeMap::new();
    for session in &snapshot.sessions {
        if let Some(selection) = selected.get(&session.exe.to_ascii_lowercase()) {
            if session.root_pid != 0 {
                targets
                    .entry(session.root_pid)
                    .or_insert_with(|| AppTarget {
                        root_pid: session.root_pid,
                        exe: session.exe.clone(),
                        name: if selection.name.is_empty() {
                            session.name.clone()
                        } else {
                            selection.name.clone()
                        },
                        created_at: processes
                            .get(&session.root_pid)
                            .and_then(|process| process.created_at),
                    });
            }
        }
    }
    let ids: BTreeSet<_> = targets.keys().copied().collect();
    targets
        .into_values()
        .filter(|target| {
            let mut pid = target.root_pid;
            let mut visited = BTreeSet::from([pid]);
            while let Some(parent) = processes
                .get(&pid)
                .and_then(|process| processes.get(&process.parent_pid))
                .copied()
            {
                if !visited.insert(parent.pid) {
                    break;
                }
            if matches!((parent.created_at, processes.get(&pid).and_then(|child| child.created_at)), (Some(parent), Some(child)) if parent > child) { break; }
                if ids.contains(&parent.pid) {
                    return false;
                }
                pid = parent.pid;
            }
            true
        })
        .collect()
}

fn session_icon(icon_path: &str, executable: Option<&Path>) -> Option<String> {
    let explicit = icon_location(icon_path);
    explicit
        .and_then(|(path, index)| extract_icon_png(&path, index))
        .or_else(|| executable.and_then(|path| extract_icon_png(path, 0)))
}

fn icon_location(value: &str) -> Option<(PathBuf, i32)> {
    let value = value.trim().trim_start_matches('@');
    if value.is_empty() {
        return None;
    }
    let (path, index) = value
        .rsplit_once(',')
        .and_then(|(path, index)| index.trim().parse::<i32>().ok().map(|index| (path, index)))
        .unwrap_or((value, 0));
    let path = PathBuf::from(path.trim().trim_matches('"'));
    local_resource_path(&path).then_some((path, index))
}

fn local_resource_path(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    let lowered = text.to_ascii_lowercase();
    !lowered.contains("://")
        && (!text.starts_with("\\\\")
            || (text.starts_with("\\\\?\\") && !lowered.starts_with("\\\\?\\unc\\")))
}

fn encode_icon_rgba(rgba: &[u8], width: u32, height: u32) -> Option<String> {
    if width == 0
        || height == 0
        || width > 256
        || height > 256
        || rgba.len() != width as usize * height as usize * 4
    {
        return None;
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(STANDARD.encode(bytes))
}

#[allow(clippy::chunks_exact_to_as_chunks)]
fn extract_icon_png(path: &Path, index: i32) -> Option<String> {
    if !local_resource_path(path) {
        return None;
    }
    use windows::{
        core::PCWSTR,
        Win32::{
            Graphics::Gdi::{
                CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush,
                SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
                HGDIOBJ,
            },
            UI::{
                Shell::ExtractIconExW,
                WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_MASK, DI_NORMAL, HICON},
            },
        },
    };
    struct Icon(HICON);
    impl Drop for Icon {
        fn drop(&mut self) {
            let _ = unsafe { DestroyIcon(self.0) };
        }
    }
    struct Dc(HDC);
    impl Drop for Dc {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteDC(self.0);
            }
        }
    }
    struct Bitmap {
        bitmap: HBITMAP,
        dc: HDC,
        previous: HGDIOBJ,
    }
    impl Drop for Bitmap {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc, self.previous);
                let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            }
        }
    }
    let wide: Vec<_> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut handle = HICON::default();
    if unsafe { ExtractIconExW(PCWSTR(wide.as_ptr()), index, Some(&mut handle), None, 1) } == 0
        || handle.is_invalid()
    {
        return None;
    }
    let icon = Icon(handle);
    let dc = Dc(unsafe { CreateCompatibleDC(None) });
    if dc.0.is_invalid() {
        return None;
    }
    let format = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: 32,
            biHeight: -32,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    let mut pixels = std::ptr::null_mut();
    let bitmap =
        unsafe { CreateDIBSection(Some(dc.0), &format, DIB_RGB_COLORS, &mut pixels, None, 0) }
            .ok()?;
    let previous = unsafe { SelectObject(dc.0, HGDIOBJ(bitmap.0)) };
    let _bitmap = Bitmap {
        bitmap,
        dc: dc.0,
        previous,
    };
    if previous.is_invalid() {
        return None;
    }
    if pixels.is_null() {
        return None;
    }
    let data = unsafe { std::slice::from_raw_parts_mut(pixels.cast::<u8>(), 32 * 32 * 4) };
    data.fill(0);
    unsafe { DrawIconEx(dc.0, 0, 0, icon.0, 32, 32, 0, None, DI_NORMAL) }.ok()?;
    unsafe {
        let _ = GdiFlush();
    }
    let mut rgba = data.to_vec();
    // Older icons use an AND mask rather than a meaningful alpha channel.
    if rgba.chunks_exact(4).all(|pixel| pixel[3] == 0) {
        data.fill(0);
        unsafe { DrawIconEx(dc.0, 0, 0, icon.0, 32, 32, 0, None, DI_MASK) }.ok()?;
        unsafe {
            let _ = GdiFlush();
        }
        for (pixel, mask) in rgba.chunks_exact_mut(4).zip(data.chunks_exact(4)) {
            pixel[3] = if mask[0] == 255 { 0 } else { 255 };
        }
    }
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        // GDI alpha icons are premultiplied; PNG uses straight alpha.
        if (1..255).contains(&pixel[3]) {
            for color in 0..3 {
                pixel[color] = ((u32::from(pixel[color]) * 255 + u32::from(pixel[3]) / 2)
                    / u32::from(pixel[3]))
                .min(255) as u8;
            }
        }
    }
    encode_icon_rgba(&rgba, 32, 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent: u32, exe: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: parent,
            exe: exe.into(),
            path: None,
            created_at: Some(u64::from(pid)),
        }
    }
    fn session(pid: u32, root: u32, exe: &str, peak: f32) -> AudioSession {
        AudioSession {
            pid,
            root_pid: root,
            exe: exe.into(),
            name: display_name(exe),
            icon_png: None,
            peak,
            active: true,
        }
    }
    fn selection(exe: &str) -> CaptureApp {
        CaptureApp {
            exe: exe.into(),
            name: display_name(exe),
            ..CaptureApp::default()
        }
    }

    #[test]
    fn tree_roots_restarts_and_overlapping_selections_do_not_duplicate_audio() {
        let mut snapshot = SessionSnapshot {
            processes: vec![
                process(10, 0, "chrome.exe"),
                process(11, 10, "Chrome.exe"),
                process(20, 0, "chrome.exe"),
                process(21, 20, "chrome.exe"),
                process(30, 10, "player.exe"),
            ],
            sessions: vec![
                session(11, 10, "chrome.exe", 0.3),
                session(21, 20, "chrome.exe", 0.2),
                session(30, 30, "player.exe", 0.1),
            ],
        };
        assert_eq!(root_process(11, &snapshot.processes), 10);
        assert_eq!(root_process(21, &snapshot.processes), 20);
        assert_eq!(
            selected_targets(
                &[selection("CHROME.EXE"), selection("player.exe")],
                &snapshot
            )
            .iter()
            .map(|target| target.root_pid)
            .collect::<Vec<_>>(),
            [10, 20]
        );
        snapshot.processes = vec![process(40, 0, "chrome.exe"), process(41, 40, "chrome.exe")];
        snapshot.sessions = vec![session(41, 40, "chrome.exe", 0.4)];
        assert_eq!(
            selected_targets(&[selection("chrome.exe")], &snapshot)[0].root_pid,
            40
        );
        assert!(selected_targets(&[], &snapshot).is_empty());
    }
}
