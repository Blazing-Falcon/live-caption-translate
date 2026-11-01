use lt_core::{
    config::OpenAiConfig,
    engines::{TranslateRequest, TranslationControl, TranslationOut, Translator},
    error::Result,
    types::UtteranceId,
};
use lt_web::OpenAiTranslator;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

type Request = (String, Value);
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
}
impl Server {
    fn new(replies: Vec<(u16, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "http://localhost:{}/v1",
            listener.local_addr().unwrap().port()
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let task = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            for (status, body) in replies {
                let mut socket = loop {
                    if stopped.load(Ordering::Acquire) {
                        return;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "client did not connect");
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                seen.lock().unwrap().push(read_request(&mut socket));
                let kind = if status == 200 {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                write!(socket, "HTTP/1.1 {status} Reply\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            task: Some(task),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}
fn read_request(socket: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let end = loop {
        if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break at + 4;
        }
        let n = socket.read(&mut buffer).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
    };
    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    while bytes.len() < end + length {
        let n = socket.read(&mut buffer).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
    }
    (
        headers,
        serde_json::from_slice(&bytes[end..end + length]).unwrap(),
    )
}
fn sse(parts: &[&str]) -> String {
    let mut out = String::from(": comment\r\n\r\n");
    for text in parts {
        out.push_str(&format!(
            "data: {}\r\n\r\n",
            json!({"choices":[{"index":0,"delta":{"content":text}}]})
        ));
    }
    out.push_str("data: [DONE]\r\n\r\n");
    out
}
fn config(server: &Server, key: &str) -> OpenAiConfig {
    OpenAiConfig {
        base_url: format!("{}/", server.url),
        model: "test-model".into(),
        api_key: key.into(),
        ..Default::default()
    }
}
fn translate(
    translator: &mut OpenAiTranslator,
    callback: &mut dyn FnMut(&str),
) -> Result<TranslationOut> {
    translator.translate(
        &TranslateRequest {
            id: UtteranceId(1),
            text: "今天的会议改到下午三点。",
            max_tokens: None,
            control: TranslationControl {
                deadline: Instant::now() + Duration::from_secs(10),
                cancelled: Arc::new(AtomicBool::new(false)),
                abort: Arc::new(AtomicBool::new(false)),
            },
        },
        callback,
    )
}

#[test]
fn streams_in_order_and_sends_only_compatible_fields_and_optional_bearer() {
    for key in ["", "test-secret"] {
        let server = Server::new(vec![(200, sse(&["The meeting", " is at 3 p.m."]))]);
        let mut cfg = config(&server, key);
        cfg.prompt = "Translate into English.".into();
        let mut translator = OpenAiTranslator::new(&cfg).unwrap();
        let mut deltas = Vec::new();
        assert_eq!(
            translate(&mut translator, &mut |text| deltas.push(text.to_owned()))
                .unwrap()
                .text,
            "The meeting is at 3 p.m."
        );
        assert_eq!(deltas, ["The meeting", "The meeting is at 3 p.m."]);
        let requests = server.requests.lock().unwrap();
        let (headers, body) = &requests[0];
        assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert_eq!(
            headers.to_lowercase().contains("authorization:"),
            !key.is_empty()
        );
        if !key.is_empty() {
            assert!(headers.contains("Bearer test-secret"));
        }
        assert_eq!(
            body,
            &json!({"model":"test-model", "messages":[{"role":"system","content":"Translate into English."},{"role":"user","content":"今天的会议改到下午三点。"}],"stream":true,"temperature":0})
        );
    }
}

#[test]
fn retries_temperature_once_and_remembers_omission_and_complete_url() {
    let server = Server::new(vec![
        (400, "{\"error\":\"temperature unsupported\"}".into()),
        (200, sse(&["The meeting is at 3 p.m."])),
        (200, sse(&["The meeting is at 3 p.m."])),
    ]);
    let mut cfg = config(&server, "");
    cfg.base_url = format!("{}/chat/completions/", server.url);
    let mut translator = OpenAiTranslator::new(&cfg).unwrap();
    translate(&mut translator, &mut |_| {}).unwrap();
    translate(&mut translator, &mut |_| {}).unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].1["temperature"], 0);
    assert!(requests[1].1.get("temperature").is_none());
    assert!(requests[2].1.get("temperature").is_none());
    assert!(requests
        .iter()
        .all(|(headers, _)| headers.starts_with("POST /v1/chat/completions HTTP/1.1")));
    assert_eq!(
        requests[1].1["messages"][0]["content"],
        "Translate the user's Chinese text into natural English. Output only the translation."
    );
}

#[test]
fn errors_name_settings_without_echoing_response_secrets() {
    for (status, expected) in [
        (401, "API key"),
        (403, "API key"),
        (404, "base URL and model"),
        (400, "HTTP 400"),
    ] {
        let server = Server::new(vec![(status, "private-echo-secret".into())]);
        let mut translator = OpenAiTranslator::new(&config(&server, "test-secret")).unwrap();
        let error = translate(&mut translator, &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("secret"));
    }
}

#[test]
fn temperature_refusal_is_not_retried_twice() {
    let refusal = "{\"error\":\"temperature unsupported\"}".to_owned();
    let server = Server::new(vec![(400, refusal.clone()), (400, refusal)]);
    let mut translator = OpenAiTranslator::new(&config(&server, "")).unwrap();
    assert!(translate(&mut translator, &mut |_| {}).is_err());
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[test]
fn premature_stream_end_and_unusable_answers_fail() {
    for body in [
        "data: {\"choices\":[]}\n\n".to_owned(),
        sse(&[]),
        sse(&["Here is the translation: Meeting."]),
    ] {
        let server = Server::new(vec![(200, body)]);
        let mut translator = OpenAiTranslator::new(&config(&server, "")).unwrap();
        assert!(translate(&mut translator, &mut |_| {}).is_err());
    }
}
