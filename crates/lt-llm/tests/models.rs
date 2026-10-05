//! Loopback HTTP + real files exercise resume, verification, import and cancellation.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use lt_core::error::Error;
use lt_llm::models::{FileEntry, Manifest, ModelEntry, ModelManager, ModelSource, ModelState};
use sha2::{Digest, Sha256};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
struct TempDir {
    root: PathBuf,
    path: PathBuf,
}
impl TempDir {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let path = root.join("target/tmp").join(format!(
            "models-test-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self { root, path }
    }
    fn final_path(&self) -> PathBuf {
        self.path.join("test-model/model.bin")
    }
    fn part_path(&self) -> PathBuf {
        self.path.join("test-model/model.bin.part")
    }
    fn write_part(&self, bytes: &[u8]) {
        fs::create_dir_all(self.path.join("test-model")).unwrap();
        fs::write(self.part_path(), bytes).unwrap();
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        if let Ok(path) = self.path.canonicalize() {
            if path.starts_with(self.root.join("target/tmp")) {
                let _ = fs::remove_dir_all(path);
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Request {
    path: String,
    range: Option<usize>,
    number: usize,
}
struct MockServer {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl MockServer {
    fn new(mut handler: impl FnMut(&mut TcpStream, &Request) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_requests = Arc::clone(&requests);
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            while !worker_stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut reader = BufReader::new(&mut stream);
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() {
                            continue;
                        }
                        let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                        let mut range = None;
                        loop {
                            line.clear();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                                break;
                            }
                            if let Some((name, value)) = line.split_once(':') {
                                if name.eq_ignore_ascii_case("range") {
                                    range = value
                                        .trim()
                                        .strip_prefix("bytes=")
                                        .and_then(|range| range.strip_suffix('-'))
                                        .and_then(|offset| offset.parse().ok());
                                }
                            }
                        }
                        drop(reader);
                        let request = Request {
                            path,
                            range,
                            number: worker_requests.lock().unwrap().len(),
                        };
                        worker_requests.lock().unwrap().push(request.clone());
                        handler(&mut stream, &request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url,
            requests,
            stopped,
            worker: Some(worker),
        }
    }
}
impl Drop for MockServer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn headers(stream: &mut TcpStream, status: u16, length: usize, range: Option<&str>) {
    let range = range.map_or_else(String::new, |range| format!("Content-Range: {range}\r\n"));
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Length: {length}\r\n{range}Connection: close\r\n\r\n"
    );
}
fn serve(stream: &mut TcpStream, request: &Request, data: &[u8]) {
    let start = request.range.unwrap_or(0);
    if start >= data.len() {
        headers(stream, 416, 0, Some(&format!("bytes */{}", data.len())));
        return;
    }
    let range = request
        .range
        .map(|_| format!("bytes {start}-{}/{}", data.len() - 1, data.len()));
    headers(
        stream,
        if request.range.is_some() { 206 } else { 200 },
        data.len() - start,
        range.as_deref(),
    );
    let _ = stream.write_all(&data[start..]);
}
fn file(name: &str, url: String, data: &[u8]) -> FileEntry {
    FileEntry {
        name: name.into(),
        size: data.len() as u64,
        sha256: format!("{:x}", Sha256::digest(data)),
        url,
        mirror: String::new(),
    }
}
fn manifest(url: String, data: &[u8]) -> Manifest {
    Manifest {
        manifest_version: 1,
        models: vec![ModelEntry {
            id: "test-model".into(),
            name: "Test model".into(),
            files: vec![file("model.bin", url, data)],
        }],
    }
}
fn manager(manifest: Manifest) -> ModelManager {
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(1)))
        .timeout_recv_response(Some(Duration::from_secs(1)))
        .timeout_recv_body(Some(Duration::from_millis(500)))
        .max_idle_connections(0)
        .build()
        .into();
    ModelManager::with_manifest_and_agent(manifest, agent).unwrap()
}

#[test]
fn interrupted_http_body_resumes_from_exact_saved_offset_and_verifies_before_rename() {
    let temp = TempDir::new();
    let data = vec![73_u8; 20_000];
    let served = data.clone();
    let server = MockServer::new(move |stream, request| {
        if request.number == 0 {
            headers(stream, 200, served.len(), None);
            stream.write_all(&served[..4_000]).unwrap();
        } else {
            serve(stream, request, &served);
        }
    });
    let manager = manager(manifest(format!("{}/model", server.url), &data));
    let mut states = Vec::new();
    let statuses = manager
        .fetch(
            ModelSource::Huggingface,
            &temp.path,
            &AtomicBool::new(false),
            &mut |status| states.push(status.state),
        )
        .unwrap();
    assert_eq!(statuses[0].state, ModelState::Ready);
    assert_eq!(fs::read(temp.final_path()).unwrap(), data);
    assert!(!temp.part_path().exists());
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].range, None);
    assert_eq!(requests[1].range, Some(4_000));
    assert!(states.contains(&ModelState::Verifying));
    assert_eq!(states.last(), Some(&ModelState::Ready));
}

#[test]
fn paused_download_keeps_partial_bytes_then_resumes() {
    let temp = TempDir::new();
    let data = vec![19_u8; 512_000];
    let served = data.clone();
    let server = MockServer::new(move |stream, request| {
        if request.number == 0 {
            headers(stream, 200, served.len(), None);
            for chunk in served.chunks(16_000) {
                if stream.write_all(chunk).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        } else {
            serve(stream, request, &served);
        }
    });
    let manager = manager(manifest(format!("{}/model", server.url), &data));
    let cancelled = AtomicBool::new(false);
    let result = manager.fetch(
        ModelSource::Huggingface,
        &temp.path,
        &cancelled,
        &mut |status| {
            if status.state == ModelState::Downloading && status.bytes_done >= 16_000 {
                cancelled.store(true, Ordering::Release);
            }
        },
    );
    assert!(matches!(result, Err(Error::Stopped)));
    assert!(!temp.final_path().exists());
    let saved = fs::metadata(temp.part_path()).unwrap().len();
    assert!(saved > 0 && saved < data.len() as u64);
    assert_eq!(
        manager.status(&temp.path).unwrap()[0].state,
        ModelState::Paused
    );
    cancelled.store(false, Ordering::Release);
    manager
        .fetch(
            ModelSource::Huggingface,
            &temp.path,
            &cancelled,
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(
        server.requests.lock().unwrap()[1].range,
        Some(saved as usize)
    );
    assert_eq!(fs::read(temp.final_path()).unwrap(), data);
}

#[test]
fn body_budget_timeout_retries_the_saved_prefix_instead_of_restarting_it() {
    let temp = TempDir::new();
    let data = vec![44_u8; 256_000];
    let served = data.clone();
    let server = MockServer::new(move |stream, request| {
        if request.number == 0 {
            headers(stream, 200, served.len(), None);
            for chunk in served.chunks(16_000) {
                if stream.write_all(chunk).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        } else {
            serve(stream, request, &served);
        }
    });
    manager(manifest(format!("{}/model", server.url), &data))
        .fetch(
            ModelSource::Huggingface,
            &temp.path,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
    let requests = server.requests.lock().unwrap();
    assert!(requests.len() >= 2);
    assert!(requests[1]
        .range
        .is_some_and(|offset| offset > 0 && offset < data.len()));
    assert_eq!(fs::read(temp.final_path()).unwrap(), data);
}

#[test]
fn range_not_satisfiable_cannot_promote_an_incomplete_partial() {
    let temp = TempDir::new();
    let data = vec![22_u8; 200];
    temp.write_part(&data[..100]);
    let server = MockServer::new(|stream, _| headers(stream, 416, 0, Some("bytes */200")));
    assert!(manager(manifest(format!("{}/model", server.url), &data))
        .fetch(
            ModelSource::Huggingface,
            &temp.path,
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .is_err());
    assert_eq!(fs::read(temp.part_path()).unwrap(), data[..100]);
    assert!(!temp.final_path().exists());
}

#[test]
fn complete_partial_is_verified_and_promoted_without_another_network_call() {
    let temp = TempDir::new();
    let data = vec![22_u8; 200];
    temp.write_part(&data);
    let server = MockServer::new(|_, _| panic!("complete partial should not request HTTP"));
    manager(manifest(format!("{}/model", server.url), &data))
        .fetch(
            ModelSource::Huggingface,
            &temp.path,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(fs::read(temp.final_path()).unwrap(), data);
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn ignored_range_restarts_file_and_invalid_range_never_changes_the_saved_prefix() {
    let temp = TempDir::new();
    let data = vec![31_u8; 2_000];
    temp.write_part(&data[..100]);
    let served = data.clone();
    let server = MockServer::new(move |stream, _| {
        headers(stream, 200, served.len(), None);
        stream.write_all(&served).unwrap();
    });
    manager(manifest(format!("{}/model", server.url), &data))
        .fetch(
            ModelSource::Huggingface,
            &temp.path,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(fs::read(temp.final_path()).unwrap(), data);
    assert_eq!(server.requests.lock().unwrap()[0].range, Some(100));

    let invalid = TempDir::new();
    invalid.write_part(&data[..100]);
    let server =
        MockServer::new(|stream, _| headers(stream, 206, 1_899, Some("bytes 101-1999/2000")));
    assert!(manager(manifest(format!("{}/model", server.url), &data))
        .fetch(
            ModelSource::Huggingface,
            &invalid.path,
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .is_err());
    assert_eq!(fs::read(invalid.part_path()).unwrap(), data[..100]);
    assert!(!invalid.final_path().exists());
}

#[test]
fn corrupt_final_and_corrupt_response_are_redownloaded_with_sha256_checks() {
    let temp = TempDir::new();
    let data = vec![7_u8; 1_000];
    fs::create_dir_all(temp.path.join("test-model")).unwrap();
    fs::write(temp.final_path(), vec![0_u8; data.len()]).unwrap();
    let served = data.clone();
    let server = MockServer::new(move |stream, request| {
        if request.number == 0 {
            headers(stream, 200, served.len(), None);
            stream.write_all(&vec![0_u8; served.len()]).unwrap();
        } else {
            serve(stream, request, &served);
        }
    });
    let manager = manager(manifest(format!("{}/model", server.url), &data));
    // Startup is deliberately size-only; fetch hashes existing files before skipping.
    assert_eq!(
        manager.status(&temp.path).unwrap()[0].state,
        ModelState::Ready
    );
    let mut states = Vec::new();
    manager
        .fetch(
            ModelSource::Huggingface,
            &temp.path,
            &AtomicBool::new(false),
            &mut |status| states.push(status.state),
        )
        .unwrap();
    assert!(states.contains(&ModelState::Corrupt));
    assert_eq!(server.requests.lock().unwrap().len(), 2);
    assert_eq!(fs::read(temp.final_path()).unwrap(), data);
}

#[test]
fn existing_flat_and_nested_files_are_hashed_and_copied_without_http() {
    let from = TempDir::new();
    let into = TempDir::new();
    let data = vec![15_u8; 2_000];
    let server = MockServer::new(|_, _| panic!("import must not use HTTP"));
    let manager = manager(manifest(format!("{}/model", server.url), &data));
    fs::write(from.path.join("model.bin"), &data).unwrap();
    let statuses = manager
        .use_existing(&from.path, &into.path, &AtomicBool::new(false), &mut |_| {})
        .unwrap();
    assert_eq!(statuses[0].state, ModelState::Ready);
    assert_eq!(fs::read(into.final_path()).unwrap(), data);
    fs::remove_file(from.path.join("model.bin")).unwrap();
    from.write_part(&[]);
    fs::write(from.final_path(), &data).unwrap();
    let nested_into = TempDir::new();
    manager
        .use_existing(
            &from.path,
            &nested_into.path,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(fs::read(nested_into.final_path()).unwrap(), data);
    fs::write(from.final_path(), vec![0_u8; data.len()]).unwrap();
    assert!(manager
        .use_existing(
            &from.path,
            &TempDir::new().path,
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .is_err());
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn downloads_are_globally_smallest_first_and_missing_mirrors_are_disabled() {
    let temp = TempDir::new();
    let server = MockServer::new(|stream, request| {
        let size = match request.path.as_str() {
            "/tiny" => 3,
            "/small" => 5,
            "/large" => 11,
            "/largest" => 19,
            _ => panic!("unexpected path"),
        };
        serve(stream, request, &vec![size as u8; size]);
    });
    let manifest = Manifest {
        manifest_version: 1,
        models: vec![
            ModelEntry {
                id: "asr".into(),
                name: "ASR".into(),
                files: vec![
                    file("large.bin", format!("{}/large", server.url), &[11; 11]),
                    file("tiny.bin", format!("{}/tiny", server.url), &[3; 3]),
                ],
            },
            ModelEntry {
                id: "vad".into(),
                name: "VAD".into(),
                files: vec![file("small.bin", format!("{}/small", server.url), &[5; 5])],
            },
            ModelEntry {
                id: "mt".into(),
                name: "MT".into(),
                files: vec![file(
                    "largest.bin",
                    format!("{}/largest", server.url),
                    &[19; 19],
                )],
            },
        ],
    };
    let manager = manager(manifest);
    assert!(!manager.source_supported(ModelSource::Modelscope));
    assert!(manager
        .fetch(
            ModelSource::Modelscope,
            &temp.path,
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .unwrap_err()
        .to_string()
        .contains("verified ModelScope"));
    manager
        .fetch(
            ModelSource::Huggingface,
            &temp.path,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
    let paths: Vec<_> = server
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.path.clone())
        .collect();
    assert_eq!(paths, ["/tiny", "/small", "/large", "/largest"]);
}

#[test]
fn unsafe_manifest_paths_are_rejected_before_any_file_or_request() {
    let data = [1_u8; 100];
    for name in [
        "../escape",
        "a/b",
        "a\\b",
        "C:\\escape",
        "NUL",
        "model.bin.part",
        "bad.",
    ] {
        let mut manifest = manifest("http://127.0.0.1/model".into(), &data);
        manifest.models[0].files[0].name = name.into();
        assert!(ModelManager::with_manifest(manifest).is_err());
    }
    let mut manifest = manifest("http://127.0.0.1/model".into(), &data);
    manifest.models[0].id = "../escape".into();
    assert!(ModelManager::with_manifest(manifest).is_err());
}

#[cfg(unix)]
#[test]
fn a_model_directory_link_cannot_escape_the_selected_directory() {
    let selected = TempDir::new();
    let outside = TempDir::new();
    std::os::unix::fs::symlink(&outside.path, selected.path.join("test-model")).unwrap();
    let manager = manager(manifest("http://127.0.0.1/model".into(), &[1; 10]));
    assert!(manager.status(&selected.path).is_err());
    assert!(manager
        .fetch(
            ModelSource::Huggingface,
            &selected.path,
            &AtomicBool::new(false),
            &mut |_| {}
        )
        .is_err());
    assert!(fs::read_dir(&outside.path).unwrap().next().is_none());
}
