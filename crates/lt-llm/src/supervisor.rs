//! Owns the local translator process and its crash-recovery budget.
use lt_core::{
    bus::EventBus,
    config::TranslateConfig,
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

#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub binary: PathBuf,
    pub model: PathBuf,
    pub threads: u32,
    pub context: u32,
    pub translate: TranslateConfig,
    pub load_timeout: Duration,
    #[cfg(test)]
    mock_directory: Option<PathBuf>,
}

impl ServerOptions {
    pub fn new(
        binary: PathBuf,
        model: PathBuf,
        config: &TranslateConfig,
        physical_cores: usize,
    ) -> Self {
        let threads = if config.threads == 0 {
            physical_cores.saturating_sub(1).clamp(1, 4) as u32
        } else {
            config.threads.clamp(1, 4)
        };
        Self {
            binary,
            model,
            threads,
            context: config.ctx,
            translate: config.clone(),
            load_timeout: Duration::from_secs(60),
            #[cfg(test)]
            mock_directory: None,
        }
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
        command
            .arg("-m")
            .arg(&self.model)
            .args(["--host", "127.0.0.1", "--port"])
            .arg(port.to_string())
            .args(["--jinja", "-np", "1", "-c"])
            .arg(self.context.to_string())
            .arg("-t")
            .arg(self.threads.to_string())
            .arg("-tb")
            .arg(self.threads.to_string())
            .args([
                "--override-kv",
                "tokenizer.ggml.eos_token_id=int:120020",
                "--no-webui",
            ]);
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
            status(&bus, EngineState::Failed, Some(message.clone()));
            return Err(Error::Engine(message));
        }
        if !options.model.is_file() {
            let message = format!("Translation model is missing: {}", options.model.display());
            status(&bus, EngineState::Failed, Some(message.clone()));
            return Err(Error::Engine(message));
        }
        let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
        let url = format!("http://127.0.0.1:{port}");
        let pid = Arc::new(AtomicU32::new(0));
        let ready = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let state = WorkerState {
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
    url: String,
    pid: Arc<AtomicU32>,
    ready: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    bus: EventBus,
}

fn status(bus: &EventBus, state: EngineState, message: Option<String>) {
    bus.publish(PipelineEvent::EngineStatus {
        engine: EngineKind::Translator,
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
            status(&state.bus, EngineState::Failed, Some(message.clone()));
            return Err(Error::Engine(message));
        }
        status(&state.bus, EngineState::Restarting, Some(message));
        if !sleep_checked(&state.cancelled, Duration::from_secs(1 << restarts)) {
            return Ok(());
        }
        restarts += 1;
    }
}

fn run_child(options: &ServerOptions, port: u16, state: &WorkerState) -> Result<()> {
    let mut owned = OwnedChild::spawn(options.command(port))?;
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
    let mut translator =
        crate::client::OpenAiCompatTranslator::new(&state.url, options.translate.clone())?;
    translator.warm_up(&control(
        &state.cancelled,
        Duration::from_secs_f64(f64::from(options.translate.timeout_s)),
    ))?;
    state.ready.store(true, Ordering::Release);
    status(&state.bus, EngineState::Ready, None);
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
    fn spawn(mut command: Command) -> Result<Self> {
        configure_process(&mut command);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| Error::Engine(format!("Could not start translator: {error}")))?;
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
fn configure_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn configure_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(any(windows, unix)))]
fn configure_process(_: &mut Command) {}

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
