//! Terminal-only end-to-end HTTP/SSE tests; no model or external service needed.
use lt_core::{
    config::TranslateConfig,
    engines::{TranslateRequest, TranslationControl, Translator},
    error::Error,
    events::FailReason,
    types::UtteranceId,
};
use lt_llm::{client::health, HyMt2Prompts, OpenAiCompatTranslator};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct MockServer {
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockServer {
    fn start(handler: impl FnOnce(TcpStream, Arc<AtomicBool>) + Send + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_millis(100)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_millis(100)))
                            .unwrap();
                        handler(stream, worker_stop);
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "client never connected to mock server"
                        );
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("mock accept: {error}"),
                }
            }
        });
        Self {
            url,
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> (String, Value) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break at + 4;
        }
        read_more(stream, &mut bytes, &mut buffer, deadline);
        assert!(bytes.len() < 32 * 1024);
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        read_more(stream, &mut bytes, &mut buffer, deadline);
    }
    let json = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
    };
    (headers, json)
}
fn read_more(stream: &mut TcpStream, bytes: &mut Vec<u8>, buffer: &mut [u8], deadline: Instant) {
    assert!(Instant::now() < deadline, "request read timed out");
    match stream.read(buffer) {
        Ok(0) => panic!("client closed before sending its request"),
        Ok(count) => bytes.extend_from_slice(&buffer[..count]),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) => {}
        Err(error) => panic!("request read: {error}"),
    }
}
fn delay(stop: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(5)),
        );
    }
}
fn plain(stream: &mut TcpStream, status: u16, body: &str) {
    write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}
fn chunk(stream: &mut TcpStream, bytes: &[u8]) {
    write!(stream, "{:x}\r\n", bytes.len()).unwrap();
    stream.write_all(bytes).unwrap();
    stream.write_all(b"\r\n").unwrap();
    stream.flush().unwrap();
}
fn control(timeout: Duration) -> TranslationControl {
    TranslationControl {
        deadline: Instant::now() + timeout,
        cancelled: Arc::new(AtomicBool::new(false)),
        abort: Arc::new(AtomicBool::new(false)),
    }
}
fn request<'a>(text: &'a str, control: TranslationControl) -> TranslateRequest<'a> {
    TranslateRequest {
        id: UtteranceId(1),
        text,
        src: "zh",
        tgt: "en",
        terms: &[],
        context: &[],
        prefill: "",
        max_tokens: None,
        control,
    }
}
fn assert_reason(
    result: lt_core::error::Result<lt_core::engines::TranslationOut>,
    expected: FailReason,
) {
    match result {
        Err(Error::Translation { reason, message }) => {
            assert_eq!(reason, expected, "actual error: {message}")
        }
        other => panic!("expected {expected:?}, actual result: {other:?}"),
    }
}

#[test]
fn exact_prompt_request_streamed_deltas_chunked_utf8_and_usage() {
    let (sent_tx, sent_rx) = mpsc::channel();
    let server = MockServer::start(move |mut stream, stop| {
        let sent = read_request(&mut stream);
        sent_tx.send(sent).unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
        chunk(&mut stream, b": heartbeat\r\n\r\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":null}}]}\r\n\r\n");
        let first = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello胖\"}}]}\n\n";
        let at = first.find('胖').unwrap() + 1;
        chunk(&mut stream, &first.as_bytes()[..at]);
        delay(&stop, Duration::from_millis(250));
        chunk(&mut stream, &first.as_bytes()[at..]);
        chunk(
            &mut stream,
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world.\"}}]}\n\n",
        );
        chunk(&mut stream, b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":48,\"completion_tokens\":9,\"prompt_tokens_details\":{\"cached_tokens\":31}}}\n\ndata: [DONE]\n\n");
        let _ = stream.write_all(b"0\r\n\r\n");
    });
    let mut translator =
        OpenAiCompatTranslator::new(&format!("{}/v1/", server.url), TranslateConfig::default())
            .unwrap();
    let mut deltas = Vec::new();
    let output = translator
        .translate(
            &request("你好。", control(Duration::from_secs(3))),
            &mut |text| deltas.push(text.to_owned()),
        )
        .unwrap();
    let (headers, body) = sent_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert_eq!(
        body,
        json!({
            "messages":[{"role":"user","content":HyMt2Prompts::user_message("你好。")}],
            "stream":true,"temperature":0.0,"repeat_penalty":1.05,"max_tokens":28,"cache_prompt":true,
            "stream_options":{"include_usage":true},
        })
    );
    assert_eq!(deltas, ["Hello胖", "Hello胖 world."]);
    assert_eq!(output.text, "Hello胖 world.");
    assert_eq!(
        (
            output.prompt_tokens,
            output.cached_tokens,
            output.generated_tokens
        ),
        (17, 31, 9)
    );
    let caps = translator.caps();
    assert!(caps.streaming);
    assert!(!caps.glossary && !caps.context);
    assert_eq!(caps.max_input_chars, 300);
    server.finish();
}

#[test]
fn llama_timings_override_generic_usage_and_multiline_sse_is_supported() {
    let server = MockServer::start(|mut stream, _| {
        read_request(&mut stream);
        plain(&mut stream, 200, "data: {\"choices\":\ndata: [{\"index\":0,\"delta\":{\"content\":\"Good.\"}}]}\n\ndata: {\"choices\":[],\"timings\":{\"prompt_n\":17,\"cache_n\":31,\"predicted_n\":9}}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":999,\"completion_tokens\":999}}\n\ndata: [DONE]\n\n");
    });
    let mut translator =
        OpenAiCompatTranslator::new(&server.url, TranslateConfig::default()).unwrap();
    let output = translator
        .translate(
            &request("你好。", control(Duration::from_secs(2))),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(output.text, "Good.");
    assert_eq!(
        (
            output.prompt_tokens,
            output.cached_tokens,
            output.generated_tokens
        ),
        (17, 31, 9)
    );
    server.finish();
}

#[test]
fn healthy_first_token_after_six_hundred_milliseconds_does_not_false_timeout() {
    let server = MockServer::start(|mut stream, stop| {
        read_request(&mut stream);
        delay(&stop, Duration::from_millis(600));
        plain(
            &mut stream,
            200,
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello.\"}}]}\n\ndata: [DONE]\n\n",
        );
    });
    let mut translator =
        OpenAiCompatTranslator::new(&server.url, TranslateConfig::default()).unwrap();
    let output = translator
        .translate(
            &request("你好。", control(Duration::from_secs(3))),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(output.text, "Hello.");
    assert_eq!(
        (
            output.prompt_tokens,
            output.cached_tokens,
            output.generated_tokens
        ),
        (0, 0, 0)
    );
    server.finish();
}

#[test]
fn cancellation_during_headers_or_partial_sse_line_finishes_within_one_second() {
    for body_started in [false, true] {
        let (entered_tx, entered_rx) = mpsc::channel();
        let server = MockServer::start(move |mut stream, stop| {
            read_request(&mut stream);
            if body_started {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial").unwrap();
            }
            entered_tx.send(()).unwrap();
            delay(&stop, Duration::from_secs(2));
        });
        let token = control(Duration::from_secs(3));
        let worker_token = token.clone();
        let url = server.url.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut translator =
                OpenAiCompatTranslator::new(&url, TranslateConfig::default()).unwrap();
            let result = translator.translate(&request("你好。", worker_token), &mut |_| {});
            result_tx.send(result).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let started = Instant::now();
        token.cancelled.store(true, Ordering::Release);
        let result = result_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert!(matches!(result, Err(Error::Stopped)));
        assert!(started.elapsed() < Duration::from_secs(1));
        server.finish();
    }
}

#[test]
fn deadline_covers_stalled_headers_and_a_body_that_never_completes() {
    for body_started in [false, true] {
        let server = MockServer::start(move |mut stream, stop| {
            read_request(&mut stream);
            if body_started {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"still incomplete").unwrap();
            }
            delay(&stop, Duration::from_secs(2));
        });
        let mut translator =
            OpenAiCompatTranslator::new(&server.url, TranslateConfig::default()).unwrap();
        let started = Instant::now();
        assert_reason(
            translator.translate(
                &request("你好。", control(Duration::from_millis(150))),
                &mut |_| {},
            ),
            FailReason::Timeout,
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        server.finish();
    }
}

#[test]
fn abort_returns_existing_streamed_text_for_the_core_runaway_guard() {
    let server = MockServer::start(|mut stream, stop| {
        read_request(&mut stream);
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Go go go \"}}]}\n\n").unwrap();
        delay(&stop, Duration::from_secs(2));
    });
    let token = control(Duration::from_secs(3));
    let abort = token.abort.clone();
    let mut translator =
        OpenAiCompatTranslator::new(&server.url, TranslateConfig::default()).unwrap();
    let output = translator
        .translate(&request("你好。", token), &mut |_| {
            abort.store(true, Ordering::Release)
        })
        .unwrap();
    assert_eq!(output.text, "Go go go ");
    server.finish();
}

#[test]
fn malformed_or_truncated_streams_are_errors_and_http_failures_are_classified() {
    for (status, body, reason) in [
        (200, "data: {invalid}\n\n", FailReason::Error),
        (
            200,
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            FailReason::ServerUnavailable,
        ),
        (503, "not ready", FailReason::ServerUnavailable),
        (400, "bad request", FailReason::Error),
    ] {
        let server = MockServer::start(move |mut stream, _| {
            read_request(&mut stream);
            plain(&mut stream, status, body);
        });
        let mut translator =
            OpenAiCompatTranslator::new(&server.url, TranslateConfig::default()).unwrap();
        assert_reason(
            translator.translate(
                &request("你好。", control(Duration::from_secs(2))),
                &mut |_| {},
            ),
            reason,
        );
        server.finish();
    }
}

#[test]
fn connection_refused_is_server_unavailable_and_remote_urls_are_rejected() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let mut translator = OpenAiCompatTranslator::new(&url, TranslateConfig::default()).unwrap();
    assert_reason(
        translator.translate(
            &request("你好。", control(Duration::from_secs(1))),
            &mut |_| {},
        ),
        FailReason::ServerUnavailable,
    );
    for invalid in [
        "https://127.0.0.1:8080",
        "http://example.com",
        "http://localhost.evil:8080",
        "http://192.168.1.1:8080",
        "http://user@127.0.0.1:8080",
        "http://127.0.0.1:0",
        "http://127.0.0.1:8080?x=1",
        "http://127.0.0.1:8080#fragment",
        "http://127.0.0.1:65536",
        "http://127.0.0.1:bad",
        "http://[::1]:65536",
        "http://127.0.0.1:",
    ] {
        assert!(
            OpenAiCompatTranslator::new(invalid, TranslateConfig::default()).is_err(),
            "{invalid}"
        );
    }
    assert!(
        OpenAiCompatTranslator::new("http://[::1]:8080/v1", TranslateConfig::default()).is_ok()
    );
    assert!(
        OpenAiCompatTranslator::new("http://localhost:8080/v1", TranslateConfig::default()).is_ok()
    );
}

#[test]
fn health_check_is_bounded_and_redirects_are_not_followed() {
    let server = MockServer::start(|mut stream, _| {
        let (headers, _) = read_request(&mut stream);
        assert!(headers.starts_with("GET /health HTTP/1.1\r\n"));
        plain(&mut stream, 200, "{}");
    });
    assert!(health(&server.url, &control(Duration::from_secs(2))).unwrap());
    server.finish();
    let server = MockServer::start(|mut stream, _| {
        read_request(&mut stream);
        stream.write_all(b"HTTP/1.1 302 Redirect\r\nLocation: http://example.com/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    assert!(!health(&server.url, &control(Duration::from_secs(2))).unwrap());
    server.finish();
}

#[test]
fn oversized_lines_and_invalid_utf8_fail_without_unbounded_buffering() {
    for invalid_utf8 in [false, true] {
        let server = MockServer::start(move |mut stream, _| {
            read_request(&mut stream);
            let body = if invalid_utf8 {
                vec![b'd', b'a', b't', b'a', b':', b' ', 255, b'\n', b'\n']
            } else {
                vec![b'x'; 65 * 1024]
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            let _ = stream.write_all(&body);
        });
        let mut translator =
            OpenAiCompatTranslator::new(&server.url, TranslateConfig::default()).unwrap();
        assert_reason(
            translator.translate(
                &request("你好。", control(Duration::from_secs(2))),
                &mut |_| {},
            ),
            FailReason::Error,
        );
        server.finish();
    }
}
