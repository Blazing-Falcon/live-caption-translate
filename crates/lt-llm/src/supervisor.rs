//! Owns the local translator process and its crash-recovery budget.
use lt_core::{
    bus::EventBus,
    config::{LatencyConfig, TranslateConfig},
    engines::{TranslationControl, Translator},
    error::{Error, Result},
    events::{EngineKind, EngineState, PipelineEvent},
};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Which model a llama-server child runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerRole {
    /// Hy-MT2: the accurate translator whose text is never rewritten.
    Final,
    /// LMT-60: quick drafts that the final replaces.
    Draft,
}

impl ServerRole {
    pub fn engine(self) -> EngineKind {
        match self {
            Self::Final => EngineKind::Translator,
            Self::Draft => EngineKind::DraftTranslator,
        }
    }

    /// Model-specific arguments; `models/manifest.json` lists the same ones (tested).
    pub fn model_args(self) -> &'static [&'static str] {
        match self {
            Self::Final => &["--override-kv", "tokenizer.ggml.eos_token_id=int:120020"],
            Self::Draft => &[],
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub role: ServerRole,
    pub binary: PathBuf,
    pub model: PathBuf,
    pub threads: u32,
    pub context: u32,
    pub translate: TranslateConfig,
    pub load_timeout: Duration,
    /// Run the child at below-normal priority (`latency.low_priority`).
    pub below_normal: bool,
    #[cfg(test)]
    mock_directory: Option<PathBuf>,
}

impl ServerOptions {
    /// A Hy-MT2 server. `_physical_cores` is unused: automatic threads mean 2 for
    /// each server.
    pub fn new(
        binary: PathBuf,
        model: PathBuf,
        config: &TranslateConfig,
        _physical_cores: usize,
    ) -> Self {
        Self {
            role: ServerRole::Final,
            binary,
            model,
            threads: config.server_threads(),
            context: config.ctx,
            translate: config.clone(),
            load_timeout: Duration::from_secs(60),
            below_normal: false,
            #[cfg(test)]
            mock_directory: None,
        }
    }

    pub fn with_role(mut self, role: ServerRole) -> Self {
        self.role = role;
        self
    }

    pub fn with_below_normal(mut self, below_normal: bool) -> Self {
        self.below_normal = below_normal;
        self
    }

    /// Every argument after the binary, in order (used by `command` and the tests).
    pub fn arguments(&self, port: u16) -> Vec<String> {
        let mut args: Vec<String> = ["-m"].map(String::from).to_vec();
        args.push(self.model.display().to_string());
        args.extend(["--host", "127.0.0.1", "--port"].map(String::from));
        args.push(port.to_string());
        args.extend(["--jinja", "-np", "1", "-c"].map(String::from));
        args.push(self.context.to_string());
        args.extend(["-t".into(), self.threads.to_string()]);
        args.extend(["-tb".into(), self.threads.to_string()]);
        args.extend(self.role.model_args().iter().map(|arg| (*arg).to_owned()));
        args.push("--no-webui".into());
        args
    }

    fn command(&self, port: u16) -> Command {
        let mut command = Command::new(&self.binary);
        #[cfg(test)]
        if let Some(directory) = &self.mock_directory {
            command
                .args([
                    "--ignored",
                    "--exact",
                    "supervisor::tests::mock_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("LT_SUPERVISOR_MOCK_PORT", port.to_string())
                .env("LT_SUPERVISOR_MOCK_DIRECTORY", directory)
                .current_dir(directory);
            return command;
        }
        command.args(self.arguments(port));
        command
    }
}

pub struct Supervisor {
    url: String,
    pid: Arc<AtomicU32>,
    ready: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl Supervisor {
    /// Returns immediately. Readiness and failures are reported on the bus.
    pub fn start(options: ServerOptions, bus: EventBus) -> Result<Self> {
        if !options.binary.is_file() {
            let message = format!("Translator binary is missing: {}", options.binary.display());
            status(
                &bus,
                options.role.engine(),
                EngineState::Failed,
                Some(message.clone()),
            );
            return Err(Error::Engine(message));
        }
        if !options.model.is_file() {
            let message = format!("Translation model is missing: {}", options.model.display());
            status(
                &bus,
                options.role.engine(),
                EngineState::Failed,
                Some(message.clone()),
            );
            return Err(Error::Engine(message));
        }
        let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
        let url = format!("http://127.0.0.1:{port}");
        let pid = Arc::new(AtomicU32::new(0));
        let ready = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let state = WorkerState {
            engine: options.role.engine(),
            url: url.clone(),
            pid: pid.clone(),
            ready: ready.clone(),
            cancelled: cancelled.clone(),
            bus,
        };
        let worker = thread::Builder::new()
            .name("lt-translator-supervisor".into())
            .spawn(move || run(options, port, state))?;
        Ok(Self {
            url,
            pid,
            ready,
            cancelled,
            worker: Some(worker),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn child_pid(&self) -> Arc<AtomicU32> {
        self.pid.clone()
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<()> {
        self.wait_ready_cancellable(timeout, &AtomicBool::new(false))
    }

    /// Allows a CLI signal to stop startup and unwind the child-process guard.
    pub fn wait_ready_cancellable(&self, timeout: Duration, cancelled: &AtomicBool) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while !self.is_ready() {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::Stopped);
            }
            if self.worker.as_ref().is_none_or(JoinHandle::is_finished) {
                return Err(Error::Engine(
                    "Translator failed to start; see engine status".into(),
                ));
            }
            if Instant::now() >= deadline {
                return Err(Error::Engine(
                    "Timed out waiting for translator readiness".into(),
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.cancelled.store(true, Ordering::Release);
        match self.worker.take() {
            Some(worker) => worker
                .join()
                .map_err(|_| Error::Engine("Translator supervisor panicked".into()))?,
            None => Ok(()),
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct WorkerState {
    engine: EngineKind,
    url: String,
    pid: Arc<AtomicU32>,
    ready: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    bus: EventBus,
}

fn status(bus: &EventBus, engine: EngineKind, state: EngineState, message: Option<String>) {
    bus.publish(PipelineEvent::EngineStatus {
        engine,
        state,
        message,
    });
}

fn control(cancelled: &Arc<AtomicBool>, timeout: Duration) -> TranslationControl {
    TranslationControl {
        deadline: Instant::now() + timeout,
        cancelled: cancelled.clone(),
        abort: Arc::new(AtomicBool::new(false)),
    }
}

fn sleep_checked(cancelled: &AtomicBool, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20)),
        );
    }
    !cancelled.load(Ordering::Acquire)
}

fn run(options: ServerOptions, port: u16, state: WorkerState) -> Result<()> {
    let mut restarts = 0usize;
    loop {
        if state.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        status(
            &state.bus,
            state.engine,
            if restarts == 0 {
                EngineState::Loading
            } else {
                EngineState::Restarting
            },
            None,
        );
        let attempt = run_child(&options, port, &state);
        state.ready.store(false, Ordering::Release);
        state.pid.store(0, Ordering::Release);
        if state.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let message = match attempt {
            Ok(()) => "Translator exited unexpectedly".into(),
            Err(error) => error.to_string(),
        };
        if restarts == 3 {
            status(
                &state.bus,
                state.engine,
                EngineState::Failed,
                Some(message.clone()),
            );
            return Err(Error::Engine(message));
        }
        status(
            &state.bus,
            state.engine,
            EngineState::Restarting,
            Some(message),
        );
        if !sleep_checked(&state.cancelled, Duration::from_secs(1 << restarts)) {
            return Ok(());
        }
        restarts += 1;
    }
}

fn run_child(options: &ServerOptions, port: u16, state: &WorkerState) -> Result<()> {
    let mut owned = OwnedChild::spawn(options.command(port), options.below_normal)?;
    state.pid.store(owned.child.id(), Ordering::Release);
    let deadline = Instant::now() + options.load_timeout;
    loop {
        if state.cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(exit) = owned.child.try_wait()? {
            return Err(Error::Engine(format!(
                "Translator exited ({exit}): {}",
                owned.last_stderr()
            )));
        }
        match crate::client::health(
            &state.url,
            &control(&state.cancelled, Duration::from_millis(800)),
        ) {
            Ok(true) => break,
            Err(Error::Stopped) => return Ok(()),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(Error::Engine(format!(
                "Translator did not become healthy within {:?}: {}",
                options.load_timeout,
                owned.last_stderr()
            )));
        }
        sleep_checked(&state.cancelled, Duration::from_millis(100));
    }
    let warm_up_control = control(
        &state.cancelled,
        Duration::from_secs_f64(f64::from(options.translate.timeout_s)),
    );
    match options.role {
        ServerRole::Final => {
            crate::client::OpenAiCompatTranslator::new(&state.url, options.translate.clone())?
                .warm_up(&warm_up_control)?;
        }
        ServerRole::Draft => {
            crate::draft::LmtDraftTranslator::new(&state.url, &LatencyConfig::default())?
                .warm_up(&warm_up_control)?;
        }
    }
    state.ready.store(true, Ordering::Release);
    status(&state.bus, state.engine, EngineState::Ready, None);
    let mut unhealthy = 0;
    loop {
        if !sleep_checked(&state.cancelled, Duration::from_millis(100)) {
            return Ok(());
        }
        if let Some(exit) = owned.child.try_wait()? {
            return Err(Error::Engine(format!(
                "Translator exited ({exit}): {}",
                owned.last_stderr()
            )));
        }
        // Health probes are small and bounded. Three consecutive failures also
        // recover an unresponsive process which has not exited.
        match crate::client::health(
            &state.url,
            &control(&state.cancelled, Duration::from_millis(800)),
        ) {
            Ok(true) => unhealthy = 0,
            Err(Error::Stopped) => return Ok(()),
            _ => unhealthy += 1,
        }
        if unhealthy >= 3 {
            return Err(Error::Engine(format!(
                "Translator stopped responding: {}",
                owned.last_stderr()
            )));
        }
    }
}

struct OwnedChild {
    child: Child,
    stderr: Arc<Mutex<VecDeque<String>>>,
    readers: Vec<JoinHandle<()>>,
    containment: Containment,
}

impl OwnedChild {
    fn spawn(mut command: Command, below_normal: bool) -> Result<Self> {
        configure_process(&mut command, below_normal);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| Error::Engine(format!("Could not start translator: {error}")))?;
        if below_normal {
            lower_priority(&child);
        }
        let containment = match Containment::attach(&child) {
            Ok(containment) => containment,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(16)));
        let mut readers = Vec::new();
        if let Some(output) = child.stdout.take() {
            readers.push(log_reader(output, None));
        }
        if let Some(output) = child.stderr.take() {
            readers.push(log_reader(output, Some(stderr.clone())));
        }
        Ok(Self {
            child,
            stderr,
            readers,
            containment,
        })
    }

    fn last_stderr(&self) -> String {
        self.stderr
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }
}

fn log_reader<R: std::io::Read + Send + 'static>(
    output: R,
    last: Option<Arc<Mutex<VecDeque<String>>>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        // Bound line allocation even if a child writes one enormous line.
        let mut reader = BufReader::new(output);
        let mut line = Vec::with_capacity(4096);
        while let Ok(buffer) = reader.fill_buf() {
            if buffer.is_empty() {
                if !line.is_empty() {
                    remember_line(&line, &last);
                }
                break;
            }
            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(buffer.len(), |at| at + 1);
            let remaining = 4096usize.saturating_sub(line.len());
            line.extend_from_slice(&buffer[..consumed.min(remaining)]);
            reader.consume(consumed);
            if newline.is_some() {
                remember_line(&line, &last);
                line.clear();
            }
        }
    })
}

fn remember_line(line: &[u8], last: &Option<Arc<Mutex<VecDeque<String>>>>) {
    let text = String::from_utf8_lossy(line).trim().to_owned();
    tracing::debug!(line = %text, "llama-server");
    let Some(last) = last else {
        return;
    };
    let Ok(mut lines) = last.lock() else {
        return;
    };
    if lines.len() == 16 {
        lines.pop_front();
    }
    lines.push_back(text);
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.containment.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

#[cfg(windows)]
fn configure_process(command: &mut Command, below_normal: bool) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};
    let mut flags = CREATE_NO_WINDOW;
    if below_normal {
        flags |= BELOW_NORMAL_PRIORITY_CLASS;
    }
    command.creation_flags(flags);
}

#[cfg(unix)]
fn configure_process(command: &mut Command, _below_normal: bool) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(any(windows, unix)))]
fn configure_process(_: &mut Command, _below_normal: bool) {}

/// Linux (development only): best effort, after the child exists.
#[cfg(unix)]
fn lower_priority(child: &Child) {
    // SAFETY: plain syscall on our own child's pid; failure is ignored.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, child.id(), 10);
    }
}

#[cfg(not(unix))]
fn lower_priority(_: &Child) {}

#[cfg(windows)]
struct Containment(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Containment {
    fn attach(child: &Child) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: owned handle, correctly-sized zero-initialized Win32 struct;
        // child handle remains alive throughout assignment.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let job = Self(handle);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(job)
        }
    }
    fn terminate(&self) {
        // SAFETY: handle is owned and valid until Drop.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Containment {
    fn drop(&mut self) {
        // SAFETY: this is the unique owned job handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
struct Containment(i32);

#[cfg(unix)]
impl Containment {
    fn attach(child: &Child) -> Result<Self> {
        Ok(Self(child.id() as i32))
    }
    fn terminate(&self) {
        // SAFETY: group was created for our child, negative PID targets only it.
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}

#[cfg(not(any(windows, unix)))]
struct Containment;
#[cfg(not(any(windows, unix)))]
impl Containment {
    fn attach(_: &Child) -> Result<Self> {
        Ok(Self)
    }
    fn terminate(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use lt_core::{engines::TranslateRequest, types::UtteranceId};
    use std::{
        io::{Read, Write},
        sync::atomic::AtomicUsize,
    };

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
    static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct TempDirectory {
        path: PathBuf,
        workspace: PathBuf,
    }
    impl TempDirectory {
        fn new() -> Self {
            let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .unwrap();
            let path = workspace.join("target/tmp").join(format!(
                "supervisor-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            assert!(path
                .canonicalize()
                .unwrap()
                .starts_with(workspace.join("target/tmp")));
            Self { path, workspace }
        }
        fn options(&self) -> ServerOptions {
            let model = self.path.join("model.mock");
            std::fs::write(&model, b"mock-only, not model data").unwrap();
            let mut options = ServerOptions::new(
                std::env::current_exe().unwrap(),
                model,
                &TranslateConfig::default(),
                4,
            );
            options.load_timeout = Duration::from_secs(3);
            options.mock_directory = Some(self.path.clone());
            options
        }
    }
    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let Ok(path) = self.path.canonicalize() else {
                return;
            };
            if path.starts_with(self.workspace.join("target/tmp")) {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }

    fn options_for(role: ServerRole, threads: u32) -> ServerOptions {
        let config = TranslateConfig {
            threads,
            ..TranslateConfig::default()
        };
        ServerOptions::new(
            PathBuf::from("llama-server"),
            PathBuf::from("model.gguf"),
            &config,
            8,
        )
        .with_role(role)
    }

    #[test]
    fn both_roles_build_the_expected_argument_lists() {
        let final_args = options_for(ServerRole::Final, 0).arguments(8081);
        assert_eq!(
            final_args.join(" "),
            "-m model.gguf --host 127.0.0.1 --port 8081 --jinja -np 1 -c 1024 -t 2 -tb 2 --override-kv tokenizer.ggml.eos_token_id=int:120020 --no-webui"
        );
        let draft_args = options_for(ServerRole::Draft, 0).arguments(8082);
        assert_eq!(
            draft_args.join(" "),
            "-m model.gguf --host 127.0.0.1 --port 8082 --jinja -np 1 -c 1024 -t 2 -tb 2 --no-webui"
        );
    }

    #[test]
    fn automatic_threads_are_two_per_server_and_explicit_values_apply_to_both() {
        for role in [ServerRole::Final, ServerRole::Draft] {
            assert!(options_for(role, 0)
                .arguments(1)
                .join(" ")
                .contains("-t 2 -tb 2"));
            assert!(options_for(role, 3)
                .arguments(1)
                .join(" ")
                .contains("-t 3 -tb 3"));
            // a value above the server cap is clamped like the config says
            assert!(options_for(role, 16)
                .arguments(1)
                .join(" ")
                .contains("-t 4 -tb 4"));
        }
    }

    #[test]
    fn the_manifest_lists_the_arguments_each_role_uses() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../models/manifest.json")).unwrap();
        let args_of = |role: &str| -> Vec<String> {
            manifest["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|model| model["role"] == role)
                .unwrap_or_else(|| panic!("no {role} entry"))["runtime"]["server_args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap().to_owned())
                .collect()
        };
        for (role, name) in [
            (ServerRole::Final, "translator"),
            (ServerRole::Draft, "draft_translator"),
        ] {
            // What the supervisor adds around the manifest's arguments.
            let used = options_for(role, 0).arguments(1);
            let mut expected = args_of(name);
            let mut remaining: Vec<String> = Vec::new();
            let mut skip = 0;
            for (index, arg) in used.iter().enumerate() {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                match arg.as_str() {
                    "-m" | "--host" | "--port" | "-t" | "-tb" => skip = 1,
                    "--no-webui" => {}
                    _ => remaining.push(used[index].clone()),
                }
            }
            remaining.sort();
            expected.sort();
            assert_eq!(remaining, expected, "{name}");
        }
    }

    #[test]
    fn missing_binary_and_model_report_failed_without_spawning() {
        let temp = TempDirectory::new();
        for binary_missing in [true, false] {
            let mut options = temp.options();
            if binary_missing {
                options.binary = temp.path.join("missing-server.exe");
            } else {
                options.model = temp.path.join("missing-model.gguf");
            }
            let bus = EventBus::default();
            let receiver = bus.subscribe(8);
            let error = match Supervisor::start(options, bus) {
                Ok(_) => panic!("missing asset must not start a child"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("missing"));
            assert!(matches!(
                receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
                PipelineEvent::EngineStatus {
                    state: EngineState::Failed,
                    ..
                }
            ));
        }
    }

    #[test]
    fn crash_restarts_child_warms_up_and_translates_again_then_stop_closes_it() {
        let _serial = PROCESS_TEST_LOCK.lock().unwrap();
        let temp = TempDirectory::new();
        let bus = EventBus::default();
        let receiver = bus.subscribe(128);
        let mut supervisor = Supervisor::start(temp.options(), bus).unwrap();
        supervisor.wait_ready(Duration::from_secs(5)).unwrap();
        let first_pid = supervisor.child_pid().load(Ordering::Acquire);
        assert_ne!(first_pid, 0);
        while receiver.try_recv().is_ok() {}
        let address: std::net::SocketAddr = supervisor
            .url()
            .strip_prefix("http://")
            .unwrap()
            .parse()
            .unwrap();
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .write_all(b"GET /crash HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        drop(stream);
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut restarting = false;
        let mut ready = false;
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(PipelineEvent::EngineStatus {
                    state: EngineState::Restarting,
                    ..
                }) => restarting = true,
                Ok(PipelineEvent::EngineStatus {
                    state: EngineState::Ready,
                    ..
                }) if restarting => {
                    ready = true;
                    break;
                }
                Ok(PipelineEvent::EngineStatus {
                    state: EngineState::Failed,
                    message,
                    ..
                }) => panic!("mock restart failed: {message:?}"),
                _ => {}
            }
        }
        assert!(restarting && ready);
        assert_ne!(supervisor.child_pid().load(Ordering::Acquire), first_pid);
        let mut translator = crate::client::OpenAiCompatTranslator::new(
            supervisor.url(),
            TranslateConfig::default(),
        )
        .unwrap();
        let request = TranslateRequest {
            id: UtteranceId(1),
            text: "你好。",
            src: "zh",
            tgt: "en",
            terms: &[],
            context: &[],
            prefill: "",
            max_tokens: None,
            control: control(&Arc::new(AtomicBool::new(false)), Duration::from_secs(2)),
        };
        assert_eq!(
            translator.translate(&request, &mut |_| {}).unwrap().text,
            "Hello."
        );
        let stopped = Instant::now();
        supervisor.stop().unwrap();
        assert!(stopped.elapsed() < Duration::from_secs(1));
        assert_eq!(supervisor.child_pid().load(Ordering::Acquire), 0);
        assert!(crate::client::health(
            supervisor.url(),
            &control(
                &Arc::new(AtomicBool::new(false)),
                Duration::from_millis(250)
            )
        )
        .is_err());
    }

    #[test]
    fn dropping_supervisor_joins_and_cleans_the_mock_process() {
        let _serial = PROCESS_TEST_LOCK.lock().unwrap();
        let temp = TempDirectory::new();
        let supervisor = Supervisor::start(temp.options(), EventBus::default()).unwrap();
        supervisor.wait_ready(Duration::from_secs(5)).unwrap();
        let url = supervisor.url().to_owned();
        let pid = supervisor.child_pid();
        drop(supervisor);
        assert_eq!(pid.load(Ordering::Acquire), 0);
        assert!(crate::client::health(
            &url,
            &control(
                &Arc::new(AtomicBool::new(false)),
                Duration::from_millis(250)
            )
        )
        .is_err());
    }

    #[test]
    fn stderr_capture_keeps_bounded_lines_and_the_final_unterminated_line() {
        let last = Arc::new(Mutex::new(VecDeque::new()));
        let mut output = (0..20)
            .map(|index| format!("line {index}\n"))
            .collect::<String>()
            .into_bytes();
        output.extend_from_slice(&vec![b'x'; 10_000]);
        log_reader(std::io::Cursor::new(output), Some(last.clone()))
            .join()
            .unwrap();
        let lines = last.lock().unwrap();
        assert_eq!(lines.len(), 16);
        assert_eq!(lines.front().unwrap(), "line 5");
        assert_eq!(lines.back().unwrap().len(), 4096);
    }

    /// Launched only by the supervisor unit fixtures in a hidden child process.
    #[test]
    #[ignore]
    fn mock_child() {
        let Ok(port) = std::env::var("LT_SUPERVISOR_MOCK_PORT") else {
            return;
        };
        let directory = PathBuf::from(std::env::var("LT_SUPERVISOR_MOCK_DIRECTORY").unwrap())
            .canonicalize()
            .unwrap();
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        assert!(directory.starts_with(workspace.join("target/tmp")));
        let port = port.parse::<u16>().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        for incoming in listener.incoming() {
            let mut stream = incoming.unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 1024];
            let mut header_end = None;
            while header_end.is_none() {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                header_end = bytes
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .map(|at| at + 4);
                assert!(bytes.len() < 32 * 1024);
            }
            let Some(header_end) = header_end else {
                continue;
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            if headers.starts_with("GET /crash ") {
                eprintln!("mock translator was deliberately crashed");
                std::process::exit(17);
            }
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while bytes.len() < header_end + length {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            let (kind, body) = if headers.starts_with("GET /health ") {
                ("application/json", "{\"status\":\"ok\"}")
            } else {
                assert!(headers.starts_with("POST /v1/chat/completions "));
                let _: serde_json::Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                ("text/event-stream", "data: {\"choices\":[{\"delta\":{\"content\":\"Hello.\"}}]}\n\ndata: [DONE]\n\n")
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = stream.write_all(response.as_bytes());
        }
    }
}
