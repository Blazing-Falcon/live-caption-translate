//! Below-normal priority for engine threads (`latency.low_priority`).
//!
//! An engine built through sherpa's C API starts ONNX Runtime pool threads that the API does
//! not let the caller configure. `with_new_threads_lowered` lowers every thread the process
//! starts while the engine loads; call it when no other part of the app starts threads.

/// Below-normal priority for the calling thread.
pub fn lower_current_thread_priority() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        // SAFETY: the pseudo handle of the calling thread is always valid.
        if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) } == 0 {
            tracing::warn!("Could not lower the thread priority");
        }
    }
}

/// Runs `build` and lowers the priority of each thread the process started meanwhile.
/// Returns what `build` returned and how many threads were lowered.
pub fn with_new_threads_lowered<T>(build: impl FnOnce() -> T) -> (T, usize) {
    #[cfg(windows)]
    {
        let before = windows::thread_ids();
        let value = build();
        let lowered = windows::thread_ids()
            .into_iter()
            .filter(|id| !before.contains(id))
            .filter(|id| windows::lower(*id))
            .count();
        (value, lowered)
    }
    #[cfg(not(windows))]
    {
        (build(), 0)
    }
}

#[cfg(windows)]
mod windows {
    use std::collections::BTreeSet;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            Threading::{
                GetCurrentProcessId, OpenThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
                THREAD_SET_INFORMATION,
            },
        },
    };

    pub fn thread_ids() -> BTreeSet<u32> {
        let mut ids = BTreeSet::new();
        // SAFETY: a thread snapshot of all processes; the handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return ids;
        }
        // SAFETY: plain query with no side effects.
        let process = unsafe { GetCurrentProcessId() };
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        // SAFETY: `entry` is a valid, sized THREADENTRY32 for the live snapshot.
        let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == process {
                ids.insert(entry.th32ThreadID);
            }
            // SAFETY: as above.
            more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        // SAFETY: the snapshot handle is owned here and closed once.
        unsafe { CloseHandle(snapshot) };
        ids
    }

    pub fn lower(id: u32) -> bool {
        // SAFETY: the handle is checked and closed; a thread that ended returns null.
        unsafe {
            let handle = OpenThread(THREAD_SET_INFORMATION, 0, id);
            if handle.is_null() {
                return false;
            }
            let lowered = SetThreadPriority(handle, THREAD_PRIORITY_BELOW_NORMAL) != 0;
            CloseHandle(handle);
            lowered
        }
    }
}
