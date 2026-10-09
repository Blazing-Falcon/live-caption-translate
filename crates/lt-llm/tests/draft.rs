//! Draft client against a mock llama-server: request bodies for both paths, the prefill
//! reaching the prompt, stop strings, timeouts and error mapping. No model needed.
use lt_core::{
    config::LatencyConfig,
    engines::{TranslateRequest, TranslationControl, Translator},
    error::Error,
    events::FailReason,
    types::UtteranceId,
};
use lt_llm::{LmtDraftTranslator, LmtPrompts};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{atomic::AtomicBool, mpsc, Arc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct MockServer {
    url: String,
    thread: Option<JoinHandle<()>>,
}

impl MockServer {
    fn start(handler: impl FnOnce(TcpStream) + Send + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_millis(100)))
                            .unwrap();
                        handler(stream);
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("mock accept: {error}"),
                }
            }
        });
        Self {
            url,
            thread: Some(thread),
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> (String, Value) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break at + 4;
        }
        read_more(stream, &mut bytes, &mut buffer, deadline);
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
    let json = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
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

fn reply(stream: &mut TcpStream, status: u16, body: &str) {
    write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

fn control(timeout: Duration) -> TranslationControl {
    TranslationControl {
        deadline: Instant::now() + timeout,
        cancelled: Arc::new(AtomicBool::new(false)),
        abort: Arc::new(AtomicBool::new(false)),
    }
}

fn request<'a>(
    text: &'a str,
    prefill: &'a str,
    max_tokens: Option<u32>,
    control: TranslationControl,
) -> TranslateRequest<'a> {
    TranslateRequest {
        id: UtteranceId(1),
        text,
        src: "zh",
        tgt: "en",
        terms: &[],
        context: &[],
        prefill,
        max_tokens,
        control,
    }
}

fn client(url: &str) -> LmtDraftTranslator {
    LmtDraftTranslator::new(url, &LatencyConfig::default()).unwrap()
}

#[test]
fn a_prefill_goes_to_completion_after_the_assistant_tag_and_only_the_continuation_comes_back() {
    let (sent_tx, sent_rx) = mpsc::channel();
    let server = MockServer::start(move |mut stream| {
        sent_tx.send(read_request(&mut stream)).unwrap();
        reply(
            &mut stream,
            200,
            r#"{"content":"world is round","tokens_evaluated":40,"tokens_cached":33,"tokens_predicted":3}"#,
        );
    });
    let mut translator = client(&server.url);
    let output = translator
        .translate(
            &request(
                "世界是圆的",
                "The ",
                Some(20),
                control(Duration::from_secs(3)),
            ),
            &mut |_| {},
        )
        .unwrap();
    let (headers, body) = sent_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(headers.starts_with("POST /completion HTTP/1.1\r\n"));
    let prompt = body["prompt"].as_str().unwrap();
    assert!(
        prompt.ends_with("<|im_start|>assistant\nThe "),
        "{prompt:?}"
    );
    assert_eq!(prompt, LmtPrompts::completion_prompt("世界是圆的", "The "));
    assert_eq!(body["stop"], json!(["<|im_end|>", "\n\n"]));
    assert_eq!(body["n_predict"], 20);
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["cache_prompt"], true);
    assert_eq!(body["stream"], false);
    // The reply is only the continuation; the scheduler prepends the prefill.
    assert_eq!(output.text, "world is round");
    assert_eq!(
        (
            output.prompt_tokens,
            output.cached_tokens,
            output.generated_tokens
        ),
        (40, 33, 3)
    );
}

#[test]
fn a_slow_server_times_out_at_the_draft_deadline() {
    let server = MockServer::start(move |mut stream| {
        let _ = read_request(&mut stream);
        thread::sleep(Duration::from_millis(1500));
        let _ = stream.write_all(b"");
    });
    let config = LatencyConfig {
        draft_timeout_s: 1.0,
        ..LatencyConfig::default()
    };
    let mut translator = LmtDraftTranslator::new(&server.url, &config).unwrap();
    let started = Instant::now();
    let result = translator.translate(
        &request("你好", "", None, control(Duration::from_secs(30))),
        &mut |_| {},
    );
    assert!(
        started.elapsed() < Duration::from_millis(1400),
        "{:?}",
        started.elapsed()
    );
    match result {
        Err(Error::Translation { reason, .. }) => assert_eq!(reason, FailReason::Timeout),
        other => panic!("expected a timeout, got {other:?}"),
    }
}
