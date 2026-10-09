//! Per-PID CPU/RSS sampling without enumerating unrelated processes.
//!
//! sysinfo 0.39.6's Windows `ProcessesToUpdate::Some` still takes a full
//! Toolhelp process snapshot. Direct Win32 queries avoid that scan. Linux reads
//! only `/proc/<pid>/stat` and `statm`. CPU is cumulative process time divided by
//! elapsed monotonic time, expressed as percent of one core (and may exceed 100).
//!
//! Sources: Microsoft GetProcessTimes/GetProcessMemoryInfo documentation and
//! Linux proc_pid_stat(5)/proc_pid_statm(5). A 200 ms minimum interval follows
//! sysinfo's verified Windows minimum; the pipeline normally samples once a second.

use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use lt_core::{
    error::{Error, Result},
    metrics::{ResourceSample, ResourceSampler},
};

const MINIMUM_CPU_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Copy)]
struct RawProcess {
    identity: u64,
    cpu_ns: u128,
    rss_bytes: u64,
}

struct Baseline {
    pid: u32,
    identity: u64,
    cpu_ns: u128,
    at: Instant,
}

#[derive(Default)]
struct CpuHistory {
    baseline: Option<Baseline>,
}

impl CpuHistory {
    fn observe(&mut self, pid: u32, raw: Option<RawProcess>, now: Instant) -> (f32, u32) {
        let Some(raw) = raw else {
            self.baseline = None;
            return (0.0, 0);
        };
        let rss_mb = (raw.rss_bytes / 1_048_576).min(u64::from(u32::MAX)) as u32;
        let mut usage = 0.0;
        if let Some(previous) = &self.baseline {
            if previous.pid == pid
                && previous.identity == raw.identity
                && raw.cpu_ns >= previous.cpu_ns
            {
                let elapsed = now.saturating_duration_since(previous.at);
                if elapsed < MINIMUM_CPU_INTERVAL {
                    // Keep the older baseline so frequent reads do not suppress CPU forever.
                    return (0.0, rss_mb);
                }
                let ratio =
                    (raw.cpu_ns - previous.cpu_ns) as f64 / elapsed.as_nanos() as f64 * 100.0;
                if ratio.is_finite() {
                    usage = ratio.min(f64::from(f32::MAX)) as f32;
                }
            }
        }
        self.baseline = Some(Baseline {
            pid,
            identity: raw.identity,
            cpu_ns: raw.cpu_ns,
            at: now,
        });
        (usage, rss_mb)
    }
}

/// Whole-PC CPU busy percent from consecutive (idle, total) tick readings.
#[derive(Default)]
struct SystemCpu {
    last: Option<(u64, u64)>,
}

impl SystemCpu {
    fn observe(&mut self, reading: Option<(u64, u64)>) -> f32 {
        let Some((idle, total)) = reading else {
            return 0.0;
        };
        let pct = match self.last {
            Some((last_idle, last_total)) if total > last_total && idle >= last_idle => {
                let busy = (total - last_total).saturating_sub(idle - last_idle);
                busy as f64 / (total - last_total) as f64 * 100.0
            }
            _ => 0.0,
        };
        self.last = Some((idle, total));
        pct.clamp(0.0, 100.0) as f32
    }
}

pub struct ProcessSampler {
    app_pid: u32,
    /// Zero means no supervised translator; the supervisor replaces this on restart.
    child_pid: Arc<AtomicU32>,
    /// The draft translator's server; zero when there is none.
    draft_pid: Arc<AtomicU32>,
    backend: platform::Backend,
    app_cpu: CpuHistory,
    child_cpu: CpuHistory,
    draft_cpu: CpuHistory,
    system_cpu: SystemCpu,
}

impl ProcessSampler {
    pub fn new(child_pid: Arc<AtomicU32>) -> Result<Self> {
        let backend = platform::Backend::new()?;
        let app_pid = std::process::id();
        if backend.read(app_pid).is_none() {
            return Err(Error::Engine(
                "Could not read application process counters".into(),
            ));
        }
        Ok(Self {
            app_pid,
            child_pid,
            draft_pid: Arc::new(AtomicU32::new(0)),
            backend,
            app_cpu: CpuHistory::default(),
            child_cpu: CpuHistory::default(),
            draft_cpu: CpuHistory::default(),
            system_cpu: SystemCpu::default(),
        })
    }

    /// Also sample the draft translator's server process.
    pub fn with_draft(mut self, draft_pid: Arc<AtomicU32>) -> Self {
        self.draft_pid = draft_pid;
        self
    }
}

impl ResourceSampler for ProcessSampler {
    fn sample(&mut self) -> ResourceSample {
        let child_pid = self.child_pid.load(Ordering::Acquire);
        let app = self.backend.read(self.app_pid);
        let child = if child_pid == 0 {
            None
        } else if child_pid == self.app_pid {
            app
        } else {
            self.backend.read(child_pid)
        };
        let draft_pid = self.draft_pid.load(Ordering::Acquire);
        let draft = if draft_pid == 0 {
            None
        } else {
            self.backend.read(draft_pid)
        };
        let now = Instant::now();
        let (cpu_app_pct, rss_app_mb) = self.app_cpu.observe(self.app_pid, app, now);
        let (cpu_translator_pct, rss_translator_mb) = self.child_cpu.observe(child_pid, child, now);
        let (cpu_draft_pct, rss_draft_mb) = self.draft_cpu.observe(draft_pid, draft, now);
        let cpu_system_pct = self.system_cpu.observe(self.backend.system_times());
        ResourceSample {
            cpu_app_pct,
            cpu_translator_pct,
            rss_app_mb,
            rss_translator_mb,
            cpu_system_pct,
            cpu_draft_pct,
            rss_draft_mb,
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::mem::size_of;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, STILL_ACTIVE},
        System::{
            ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::{
                GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    };

    pub struct Backend;

    struct ProcessHandle(HANDLE);
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            // SAFETY: OpenProcess returned this owned handle, closed exactly once here.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    impl Backend {
        pub fn new() -> Result<Self> {
            // Check the current process rather than hiding an unsupported/query failure.
            let backend = Self;
            if backend.read(std::process::id()).is_none() {
                return Err(Error::Engine(
                    "Could not read Win32 process counters".into(),
                ));
            }
            Ok(backend)
        }

        pub fn read(&self, pid: u32) -> Option<RawProcess> {
            // SAFETY: This opens only the specified PID with query access and no inheritance.
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if handle.is_null() {
                return None;
            }
            let handle = ProcessHandle(handle);
            let mut exit_code = 0;
            // SAFETY: Valid query handle and initialized writable DWORD pointer.
            if unsafe { GetExitCodeProcess(handle.0, &mut exit_code) } == 0
                || exit_code != STILL_ACTIVE as u32
            {
                return None;
            }
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            // SAFETY: All four FILETIME pointers refer to initialized writable storage.
            if unsafe {
                GetProcessTimes(handle.0, &mut creation, &mut exit, &mut kernel, &mut user)
            } == 0
            {
                return None;
            }
            let mut memory = PROCESS_MEMORY_COUNTERS {
                cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                ..PROCESS_MEMORY_COUNTERS::default()
            };
            let memory_size = memory.cb;
            // SAFETY: Correct structure size, valid query handle and output buffer.
            if unsafe { K32GetProcessMemoryInfo(handle.0, &mut memory, memory_size) } == 0 {
                return None;
            }
            let cpu_ticks = u128::from(filetime(kernel)) + u128::from(filetime(user));
            Some(RawProcess {
                identity: filetime(creation),
                cpu_ns: cpu_ticks * 100,
                rss_bytes: memory.WorkingSetSize as u64,
            })
        }
    }

    fn filetime(time: FILETIME) -> u64 {
        u64::from(time.dwLowDateTime) | (u64::from(time.dwHighDateTime) << 32)
    }

    impl Backend {
        /// `(idle, total)` ticks over all logical CPUs; kernel time already includes idle.
        pub fn system_times(&self) -> Option<(u64, u64)> {
            use windows_sys::Win32::System::Threading::GetSystemTimes;
            let mut idle = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            // SAFETY: three initialized writable FILETIME outputs.
            if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
                return None;
            }
            Some((filetime(idle), filetime(kernel) + filetime(user)))
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub struct Backend {
        ticks_per_second: u64,
        page_bytes: u64,
    }

    impl Backend {
        pub fn new() -> Result<Self> {
            // SAFETY: sysconf takes only its constant selector and retains no pointers.
            let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
            // SAFETY: As above, querying the OS page size has no side effects.
            let page_bytes = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            if ticks <= 0 || page_bytes <= 0 {
                return Err(Error::Engine(
                    "Could not read Linux clock/page-size counters".into(),
                ));
            }
            Ok(Self {
                ticks_per_second: ticks as u64,
                page_bytes: page_bytes as u64,
            })
        }

        pub fn read(&self, pid: u32) -> Option<RawProcess> {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
            parse(&stat, &statm, self.ticks_per_second, self.page_bytes)
        }

        /// `(idle, total)` jiffies from the aggregate `cpu` line of `/proc/stat`.
        pub fn system_times(&self) -> Option<(u64, u64)> {
            parse_system(&std::fs::read_to_string("/proc/stat").ok()?)
        }
    }

    fn parse_system(stat: &str) -> Option<(u64, u64)> {
        let line = stat.lines().next()?.strip_prefix("cpu")?;
        let values: Vec<u64> = line
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        // user nice system idle iowait irq softirq steal
        let idle = values.get(3)? + values.get(4).copied().unwrap_or(0);
        let total: u64 = values.iter().take(8).sum();
        Some((idle, total))
    }

    fn parse(
        stat: &str,
        statm: &str,
        ticks_per_second: u64,
        page_bytes: u64,
    ) -> Option<RawProcess> {
        // comm (field 2) may itself contain spaces and ')'; subsequent fields cannot.
        let (_, fields) = stat.rsplit_once(')')?;
        let fields: Vec<_> = fields.split_whitespace().collect();
        if matches!(*fields.first()?, "Z" | "X" | "x") {
            return None;
        }
        // Remaining fields begin at field 3 (state).
        let user_ticks = fields.get(11)?.parse::<u64>().ok()?;
        let kernel_ticks = fields.get(12)?.parse::<u64>().ok()?;
        let identity = fields.get(19)?.parse::<u64>().ok()?;
        let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        Some(RawProcess {
            identity,
            cpu_ns: (u128::from(user_ticks) + u128::from(kernel_ticks)) * 1_000_000_000
                / u128::from(ticks_per_second),
            rss_bytes: resident_pages.saturating_mul(page_bytes),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::*;
    pub struct Backend;
    impl Backend {
        pub fn new() -> Result<Self> {
            Err(Error::Engine(
                "Process sampling is supported on Windows and Linux".into(),
            ))
        }
        pub fn read(&self, _: u32) -> Option<RawProcess> {
            None
        }
        pub fn system_times(&self) -> Option<(u64, u64)> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_cpu_is_the_busy_share_between_readings() {
        let mut cpu = SystemCpu::default();
        assert_eq!(
            cpu.observe(Some((100, 200))),
            0.0,
            "first reading is a baseline"
        );
        // 50 ticks passed, 30 of them idle: 40% busy.
        assert!((cpu.observe(Some((130, 250))) - 40.0).abs() < 1e-3);
        assert_eq!(cpu.observe(None), 0.0);
        // counters that go backwards restart the baseline
        assert_eq!(cpu.observe(Some((10, 20))), 0.0);
    }
}
