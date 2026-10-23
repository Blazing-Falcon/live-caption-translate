//! Per-PID CPU, working-set and private-byte sampling without enumerating unrelated processes.
//!
//! sysinfo 0.39.6's Windows `ProcessesToUpdate::Some` still takes a full
//! Toolhelp process snapshot. Direct Win32 queries avoid that scan. Linux reads
//! only `/proc/<pid>/stat` and `statm`. CPU is cumulative process time divided by
//! elapsed monotonic time, expressed as percent of one core (and may exceed 100).
//!
//! The working set counts pages shared with other processes and shrinks when Windows trims it,
//! so it does not answer "how much does this process cost". Private bytes (commit charge) do.
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
    /// Private bytes: commit charge on Windows, the private data size on Linux.
    private_bytes: u64,
}

/// One reading of a process: CPU percent of one core, working set and private bytes in MiB.
#[derive(Clone, Copy, Default)]
struct Usage {
    cpu_pct: f32,
    rss_mb: u32,
    private_mb: u32,
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
    fn observe(&mut self, pid: u32, raw: Option<RawProcess>, now: Instant) -> Usage {
        let Some(raw) = raw else {
            self.baseline = None;
            return Usage::default();
        };
        let mut usage = Usage {
            rss_mb: megabytes(raw.rss_bytes),
            private_mb: megabytes(raw.private_bytes),
            ..Usage::default()
        };
        if let Some(previous) = &self.baseline {
            if previous.pid == pid
                && previous.identity == raw.identity
                && raw.cpu_ns >= previous.cpu_ns
            {
                let elapsed = now.saturating_duration_since(previous.at);
                if elapsed < MINIMUM_CPU_INTERVAL {
                    // Keep the older baseline so frequent reads do not suppress CPU forever.
                    return usage;
                }
                let ratio =
                    (raw.cpu_ns - previous.cpu_ns) as f64 / elapsed.as_nanos() as f64 * 100.0;
                if ratio.is_finite() {
                    usage.cpu_pct = ratio.min(f64::from(f32::MAX)) as f32;
                }
            }
        }
        self.baseline = Some(Baseline {
            pid,
            identity: raw.identity,
            cpu_ns: raw.cpu_ns,
            at: now,
        });
        usage
    }
}

fn megabytes(bytes: u64) -> u32 {
    (bytes / 1_048_576).min(u64::from(u32::MAX)) as u32
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

/// A llama-server child. Its pid is zero while none runs; the supervisor replaces it on restart.
struct Child {
    pid: Arc<AtomicU32>,
    cpu: CpuHistory,
}

impl Child {
    fn new(pid: Arc<AtomicU32>) -> Self {
        Self {
            pid,
            cpu: CpuHistory::default(),
        }
    }

    fn observe(
        &mut self,
        backend: &platform::Backend,
        app_pid: u32,
        app: Option<RawProcess>,
        now: Instant,
    ) -> Usage {
        let pid = self.pid.load(Ordering::Acquire);
        let raw = if pid == 0 {
            None
        } else if pid == app_pid {
            app
        } else {
            backend.read(pid)
        };
        self.cpu.observe(pid, raw, now)
    }
}

pub struct ProcessSampler {
    app_pid: u32,
    backend: platform::Backend,
    app_cpu: CpuHistory,
    translator: Child,
    speech: Child,
    system_cpu: SystemCpu,
    private_mb: (u32, u32, u32),
}

impl ProcessSampler {
    /// `translator` and `speech` are the pids of the translation and second-pass servers.
    pub fn new(translator: Arc<AtomicU32>, speech: Arc<AtomicU32>) -> Result<Self> {
        let backend = platform::Backend::new()?;
        let app_pid = std::process::id();
        if backend.read(app_pid).is_none() {
            return Err(Error::Engine(
                "Could not read application process counters".into(),
            ));
        }
        Ok(Self {
            app_pid,
            backend,
            app_cpu: CpuHistory::default(),
            translator: Child::new(translator),
            speech: Child::new(speech),
            system_cpu: SystemCpu::default(),
            private_mb: (0, 0, 0),
        })
    }

    /// Private bytes of the app, the translator and the second-pass server from the last
    /// sample, in MiB. `ResourceSample` carries only the working set, so this is where the
    /// companion counter is read out.
    pub fn private_mb(&self) -> (u32, u32, u32) {
        self.private_mb
    }
}

impl ResourceSampler for ProcessSampler {
    fn sample(&mut self) -> ResourceSample {
        let app = self.backend.read(self.app_pid);
        let now = Instant::now();
        let app_usage = self.app_cpu.observe(self.app_pid, app, now);
        let translator = self
            .translator
            .observe(&self.backend, self.app_pid, app, now);
        let speech = self.speech.observe(&self.backend, self.app_pid, app, now);
        self.private_mb = (
            app_usage.private_mb,
            translator.private_mb,
            speech.private_mb,
        );
        let cpu_system_pct = self.system_cpu.observe(self.backend.system_times());
        ResourceSample {
            cpu_app_pct: app_usage.cpu_pct,
            cpu_translator_pct: translator.cpu_pct,
            cpu_speech_pct: speech.cpu_pct,
            rss_app_mb: app_usage.rss_mb,
            rss_translator_mb: translator.rss_mb,
            rss_speech_mb: speech.rss_mb,
            cpu_system_pct,
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
            ProcessStatus::{
                K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
            },
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
            // `PrivateUsage` is commit charge, not private working set: it counts what the
            // process has committed of RAM and page file, where `WorkingSetSize` also counts
            // pages shared with other processes and drops when Windows trims.
            let mut memory = PROCESS_MEMORY_COUNTERS_EX {
                cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
                ..PROCESS_MEMORY_COUNTERS_EX::default()
            };
            // `K32GetProcessMemoryInfo` is declared for the base struct, which is the prefix of
            // the `_EX` one; `cb` tells the kernel how far it may write.
            let memory_size = memory.cb;
            // SAFETY: `_EX` starts with the fields of `PROCESS_MEMORY_COUNTERS`, and `cb` is
            // the `_EX` size, which is what the call validates before it fills the tail.
            if unsafe {
                K32GetProcessMemoryInfo(
                    handle.0,
                    &mut memory as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                    memory_size,
                )
            } == 0
            {
                return None;
            }
            let cpu_ticks = u128::from(filetime(kernel)) + u128::from(filetime(user));
            Some(RawProcess {
                identity: filetime(creation),
                cpu_ns: cpu_ticks * 100,
                rss_bytes: memory.WorkingSetSize as u64,
                private_bytes: memory.PrivateUsage as u64,
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
        // Windows reports commit charge here. Linux has no per-process commit counter; the data
        // segment (heap and stack) is the closest analogue. It undercounts allocations that come
        // from mmap, and it is not comparable with the Windows figure.
        let data_pages = statm.split_whitespace().nth(5)?.parse::<u64>().ok()?;
        Some(RawProcess {
            identity,
            cpu_ns: (u128::from(user_ticks) + u128::from(kernel_ticks)) * 1_000_000_000
                / u128::from(ticks_per_second),
            rss_bytes: resident_pages.saturating_mul(page_bytes),
            private_bytes: data_pages.saturating_mul(page_bytes),
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

    /// A wrong structure size or a wrong field makes the private bytes of every process read as
    /// zero, and no other test looks at them.
    #[cfg(windows)]
    #[test]
    fn the_current_process_reports_private_bytes_and_no_servers() {
        let no_server = Arc::new(AtomicU32::new(0));
        let mut sampler = ProcessSampler::new(no_server.clone(), no_server).unwrap();
        let sample = sampler.sample();
        let (app_mb, translator_mb, speech_mb) = sampler.private_mb();
        assert!(app_mb > 0, "the app has committed memory");
        assert!(sample.rss_app_mb > 0, "the app has a working set");
        // With no server running, their readings are zero rather than the app's.
        assert_eq!((translator_mb, speech_mb), (0, 0));
    }
}
