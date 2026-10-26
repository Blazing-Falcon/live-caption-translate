//! Loopback HTTP for the bundled server: address checks, a cancellable transport, and SSE.

use lt_core::{
    engines::TranslationControl,
    error::{Error, Result},
    events::FailReason,
};
use std::{
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use ureq::{
    http::Uri,
    unversioned::{
        resolver::{ResolvedSocketAddrs, Resolver},
        transport::{Buffers, ConnectionDetails, Connector, LazyBuffers, NextTimeout, Transport},
    },
    Agent,
};

const POLL: Duration = Duration::from_millis(100);
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_EVENT_BYTES: usize = 64 * 1024;
const HTTP_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Endpoint {
    pub(crate) address: SocketAddr,
    pub(crate) completions: String,
}

impl Endpoint {
    pub(crate) fn new(base_url: &str) -> Result<Self> {
        if base_url.contains('#') {
            return Err(Error::Config(
                "Translation server base URL cannot have a fragment".into(),
            ));
        }
        let uri: Uri = base_url
            .parse()
            .map_err(|_| Error::Config("Invalid local translation server URL".into()))?;
        if uri.query().is_some() {
            return Err(Error::Config(
                "Translation server base URL cannot have a query".into(),
            ));
        }
        let address = local_address(&uri)?;
        let base = uri.path().trim_end_matches('/');
        let path = if base.ends_with("/chat/completions") {
            base.to_owned()
        } else if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        };
        Ok(Self {
            address,
            completions: format!("http://{address}{path}"),
        })
    }
}

fn local_address(uri: &Uri) -> Result<SocketAddr> {
    if uri.scheme_str() != Some("http") {
        return Err(Error::Config(
            "Translation server must use loopback HTTP".into(),
        ));
    }
    let authority = uri
        .authority()
        .ok_or_else(|| Error::Config("Translation server URL has no authority".into()))?;
    if authority.as_str().contains('@') {
        return Err(Error::Config(
            "Translation server URL cannot contain credentials".into(),
        ));
    }
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    // The bundled server binds IPv4. Canonicalizing localhost avoids a DNS
    // helper thread and its uncancellable lookup during shutdown.
    let ip = if host.eq_ignore_ascii_case("localhost") {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        host.parse::<IpAddr>().map_err(|_| {
            Error::Config("Translation server must be localhost or a numeric loopback IP".into())
        })?
    };
    if !ip.is_loopback() {
        return Err(Error::Config(
            "Translation server address is not loopback".into(),
        ));
    }
    let explicit_port = if authority.as_str().starts_with('[') {
        authority
            .as_str()
            .split_once(']')
            .is_some_and(|(_, suffix)| !suffix.is_empty())
    } else {
        authority.as_str().contains(':')
    };
    if explicit_port && authority.port_u16().is_none() {
        return Err(Error::Config(
            "Translation server port must be an integer from 1 to 65535".into(),
        ));
    }
    let port = authority.port_u16().unwrap_or(80);
    if port == 0 {
        return Err(Error::Config(
            "Translation server port cannot be zero".into(),
        ));
    }
    Ok(SocketAddr::new(ip, port))
}

pub(crate) fn health(base_url: &str, control: &TranslationControl) -> Result<bool> {
    control.check()?;
    let endpoint = Endpoint::new(base_url)?;
    let agent = local_agent(endpoint.address, control.clone())?;
    let response = agent
        .get(format!("http://{}/health", endpoint.address))
        .call()
        .map_err(|error| map_http_error(error, control))?;
    Ok(response.status().as_u16() == 200)
}

pub(crate) fn translation_error(message: impl Into<String>) -> Error {
    Error::Translation {
        reason: FailReason::Error,
        message: message.into(),
    }
}

pub(crate) fn server_unavailable(message: impl Into<String>) -> Error {
    Error::Translation {
        reason: FailReason::ServerUnavailable,
        message: message.into(),
    }
}

pub(crate) fn map_http_error(error: ureq::Error, control: &TranslationControl) -> Error {
    if control.cancelled.load(Ordering::Relaxed) || control.abort.load(Ordering::Relaxed) {
        return control
            .check()
            .err()
            .unwrap_or_else(|| translation_error("Translation was interrupted"));
    }
    // Keep the connector's stage-specific error: a deadline before any TCP
    // connection means the local server is unavailable. A deadline after
    // connecting (including stalled headers/body) remains a translation timeout.
    if let ureq::Error::Other(inner) = error {
        return match inner.downcast::<Error>() {
            Ok(error) => *error,
            Err(_) => translation_error("Invalid response from the local translation server"),
        };
    }
    if let Err(error) = control.check() {
        return error;
    }
    match error {
        ureq::Error::Timeout(_) => Error::Translation {
            reason: FailReason::Timeout,
            message: "Translation took too long".into(),
        },
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
            server_unavailable("Cannot communicate with the local translation server")
        }
        _ => translation_error("Invalid response from the local translation server"),
    }
}

#[derive(Default)]
pub(crate) struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
}

pub(crate) enum SseEvent {
    Done,
    Data(Vec<u8>),
}

impl SseDecoder {
    pub(crate) fn byte(&mut self, byte: u8) -> Result<Option<SseEvent>> {
        if byte != b'\n' {
            if self.line.len() == MAX_LINE_BYTES {
                return Err(translation_error(
                    "Translation SSE line exceeded its size limit",
                ));
            }
            self.line.push(byte);
            return Ok(None);
        }
        if self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        if self.line.is_empty() {
            if self.data.is_empty() {
                return Ok(None);
            }
            let data = std::mem::take(&mut self.data);
            if data.as_slice() == b"[DONE]" {
                return Ok(Some(SseEvent::Done));
            }
            return Ok(Some(SseEvent::Data(data)));
        }
        let line = std::mem::take(&mut self.line);
        // Decoding after the complete line preserves partial multi-byte UTF-8.
        std::str::from_utf8(&line)
            .map_err(|_| translation_error("Invalid UTF-8 in translation SSE event"))?;
        if let Some(data) = line.strip_prefix(b"data:") {
            let data = data.strip_prefix(b" ").map_or(data, |data| data);
            let extra = usize::from(!self.data.is_empty());
            if self
                .data
                .len()
                .saturating_add(data.len())
                .saturating_add(extra)
                > MAX_EVENT_BYTES
            {
                return Err(translation_error(
                    "Translation SSE event exceeded its size limit",
                ));
            }
            if extra != 0 {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(data);
        }
        Ok(None)
    }
}

#[derive(Debug)]
struct LocalResolver {
    address: SocketAddr,
}

impl Resolver for LocalResolver {
    fn resolve(
        &self,
        uri: &Uri,
        _: &ureq::config::Config,
        _: NextTimeout,
    ) -> std::result::Result<ResolvedSocketAddrs, ureq::Error> {
        if local_address(uri).map_err(core_http_error)? != self.address {
            return Err(core_http_error(translation_error(
                "Local server address changed",
            )));
        }
        let mut addresses = self.empty();
        addresses.push(self.address);
        Ok(addresses)
    }
}

#[derive(Debug)]
struct LocalConnector {
    address: SocketAddr,
    control: TranslationControl,
}

impl Connector for LocalConnector {
    type Out = LocalTransport;

    fn connect(
        &self,
        details: &ConnectionDetails<'_>,
        _: Option<()>,
    ) -> std::result::Result<Option<Self::Out>, ureq::Error> {
        if local_address(details.uri).map_err(core_http_error)? != self.address {
            return Err(core_http_error(translation_error(
                "Local server address changed",
            )));
        }
        let until = phase_deadline(details.timeout);
        let stream = loop {
            let poll = match poll_budget(&self.control, until, details.timeout) {
                Ok(poll) => poll,
                Err(error)
                    if self.control.cancelled.load(Ordering::Relaxed)
                        || self.control.abort.load(Ordering::Relaxed) =>
                {
                    return Err(error)
                }
                Err(_) => {
                    return Err(core_http_error(server_unavailable(
                        "The local translation server did not accept a connection",
                    )))
                }
            };
            match TcpStream::connect_timeout(&self.address, poll) {
                Ok(stream) => break stream,
                Err(error) if polling_error(&error) => {}
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_nodelay(true)?;
        Ok(Some(LocalTransport {
            stream,
            buffers: LazyBuffers::new(HTTP_BUFFER_BYTES, HTTP_BUFFER_BYTES),
            control: self.control.clone(),
            open: true,
        }))
    }
}

#[derive(Debug)]
struct LocalTransport {
    stream: TcpStream,
    buffers: LazyBuffers,
    control: TranslationControl,
    open: bool,
}

impl Transport for LocalTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        &mut self.buffers
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        let until = phase_deadline(timeout);
        let mut offset = 0;
        while offset < amount {
            self.stream
                .set_write_timeout(Some(poll_budget(&self.control, until, timeout)?))?;
            match self.stream.write(&self.buffers.output()[offset..amount]) {
                Ok(0) => {
                    self.open = false;
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Local server stopped accepting data",
                    )
                    .into());
                }
                Ok(count) => offset += count,
                Err(error) if polling_error(&error) => {}
                Err(error) => {
                    self.open = false;
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }

    fn await_input(&mut self, timeout: NextTimeout) -> std::result::Result<bool, ureq::Error> {
        let until = phase_deadline(timeout);
        loop {
            self.stream
                .set_read_timeout(Some(poll_budget(&self.control, until, timeout)?))?;
            match self.stream.read(self.buffers.input_append_buf()) {
                Ok(count) => {
                    self.buffers.input_appended(count);
                    self.open = count != 0;
                    return Ok(count != 0);
                }
                Err(error) if polling_error(&error) => {}
                Err(error) => {
                    self.open = false;
                    return Err(error.into());
                }
            }
        }
    }

    fn is_open(&mut self) -> bool {
        self.open
    }
}

fn core_http_error(error: Error) -> ureq::Error {
    ureq::Error::Other(Box::new(error))
}

fn polling_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

fn phase_deadline(timeout: NextTimeout) -> Option<Instant> {
    if timeout.after.is_not_happening() {
        None
    } else {
        Instant::now().checked_add(*timeout.after)
    }
}

fn poll_budget(
    control: &TranslationControl,
    phase: Option<Instant>,
    timeout: NextTimeout,
) -> std::result::Result<Duration, ureq::Error> {
    control.check().map_err(core_http_error)?;
    let now = Instant::now();
    if phase.is_some_and(|deadline| now >= deadline) {
        return Err(ureq::Error::Timeout(timeout.reason));
    }
    let deadline = phase.map_or(control.deadline, |deadline| deadline.min(control.deadline));
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Err(ureq::Error::Timeout(timeout.reason));
    }
    Ok(remaining.min(POLL))
}

pub(crate) fn local_agent(address: SocketAddr, control: TranslationControl) -> Result<Agent> {
    control.check()?;
    let remaining = control.deadline.saturating_duration_since(Instant::now());
    let config = Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .max_response_header_size(HTTP_BUFFER_BYTES)
        .input_buffer_size(HTTP_BUFFER_BYTES)
        .output_buffer_size(HTTP_BUFFER_BYTES)
        .timeout_global(Some(remaining))
        .build();
    Ok(Agent::with_parts(
        config,
        LocalConnector { address, control },
        LocalResolver { address },
    ))
}
