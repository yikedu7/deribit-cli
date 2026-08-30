//! Generic, local-only HTTP loopback support for transport tests.
//!
//! This module owns listener setup, raw request capture, request counting, and
//! response sequencing.  It deliberately makes no assertions about transport
//! policy; the caller decides what each captured exchange means.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use url::Url;

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(2);
const CONNECTION_READ_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_REQUEST_HEADER_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

type ParsedRequestHeaders = (String, String, String, Vec<(String, String)>);

/// A generic response action for every request received by a loopback server.
#[derive(Clone, Debug)]
pub struct ResponsePlan {
    action: ResponseAction,
    headers: Vec<(String, String)>,
    head_delay: Duration,
    body_delay: Duration,
}

#[derive(Clone, Debug)]
enum ResponseAction {
    Send { status: u16, body: Vec<u8> },
    CloseWithoutResponse,
}

impl ResponsePlan {
    /// Creates an HTTP response with the supplied status and complete body.
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            action: ResponseAction::Send { status, body },
            headers: Vec::new(),
            head_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
        }
    }

    /// Creates an action that closes the accepted connection without a response.
    pub fn close_without_response() -> Self {
        Self {
            action: ResponseAction::CloseWithoutResponse,
            headers: Vec::new(),
            head_delay: Duration::ZERO,
            body_delay: Duration::ZERO,
        }
    }

    /// Adds one raw HTTP response header.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Delays response-header delivery after a complete request is captured.
    pub fn with_head_delay(mut self, delay: Duration) -> Self {
        self.head_delay = delay;
        self
    }

    /// Delays response-body delivery after headers have been flushed.
    #[allow(dead_code)]
    pub fn with_body_delay(mut self, delay: Duration) -> Self {
        self.body_delay = delay;
        self
    }

    fn write_to(&self, stream: &mut TcpStream, shutdown: &AtomicBool) -> io::Result<()> {
        if !wait_without_shutdown(self.head_delay, shutdown) {
            return Ok(());
        }

        let ResponseAction::Send { status, body } = &self.action else {
            return Ok(());
        };

        let mut response = format!("HTTP/1.1 {status} loopback\r\n");
        let has_content_length = self
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        let has_connection = self
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("connection"));

        for (name, value) in &self.headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        if !has_content_length {
            response.push_str("Content-Length: ");
            response.push_str(&body.len().to_string());
            response.push_str("\r\n");
        }
        if !has_connection {
            response.push_str("Connection: close\r\n");
        }
        response.push_str("\r\n");

        stream.write_all(response.as_bytes())?;
        stream.flush()?;
        if !wait_without_shutdown(self.body_delay, shutdown) {
            return Ok(());
        }
        stream.write_all(body)?;
        stream.flush()
    }
}

/// A raw HTTP request captured by the generic loopback server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedRequest {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl CapturedRequest {
    /// Returns the request method token.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Returns the request target exactly as received.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Returns the received HTTP version token.
    #[allow(dead_code)]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns raw request headers in wire order.
    #[allow(dead_code)]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Returns one request header by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Returns the complete raw request body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    fn read_from(stream: &mut TcpStream) -> io::Result<Self> {
        stream.set_read_timeout(Some(CONNECTION_READ_TIMEOUT))?;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            if bytes.len() > MAX_REQUEST_HEADER_BYTES {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "loopback request headers exceed the capture limit",
                ));
            }

            if let Some(position) = find_header_end(&bytes) {
                break position;
            }

            let read = stream.read(&mut buffer)?;
            if read == 0 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "loopback connection closed before request headers",
                ));
            }
            bytes.extend_from_slice(&buffer[..read]);
        };

        let header_text = std::str::from_utf8(&bytes[..header_end]).map_err(|_| {
            io::Error::new(
                ErrorKind::InvalidData,
                "loopback request headers are not valid UTF-8",
            )
        })?;
        let (method, target, version, headers) = parse_request_headers(header_text)?;
        let body_length = content_length(&headers)?;
        if body_length > MAX_REQUEST_BODY_BYTES {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "loopback request body exceeds the capture limit",
            ));
        }

        let body_end = header_end + body_length;
        while bytes.len() < body_end {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "loopback connection closed before request body",
                ));
            }
            bytes.extend_from_slice(&buffer[..read]);
        }

        Ok(Self {
            method,
            target,
            version,
            headers,
            body: bytes[header_end..body_end].to_vec(),
        })
    }
}

/// A local server that uses one generic response plan for every accepted request.
pub struct LoopbackServer {
    address: SocketAddr,
    state: Arc<ServerState>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl LoopbackServer {
    /// Binds a server to the numeric IPv4 loopback interface on an ephemeral port.
    pub fn start(response: ResponsePlan) -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let state = Arc::new(ServerState::new(response));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = thread::spawn(move || serve(listener, worker_state, worker_shutdown));

        Ok(Self {
            address,
            state,
            shutdown,
            worker: Some(worker),
        })
    }

    /// Returns the numeric loopback socket address.
    #[allow(dead_code)]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// Constructs a local HTTP URL using the supplied absolute path.
    pub fn url(&self, path: &str) -> Url {
        let path = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        Url::parse(&format!("http://{}{path}", self.address))
            .expect("numeric loopback address must form a valid URL")
    }

    /// Returns the number of complete requests captured so far.
    pub fn request_count(&self) -> usize {
        self.state.request_count.load(Ordering::SeqCst)
    }

    /// Waits until at least the specified number of complete requests is captured.
    pub fn wait_for_request_count(&self, expected: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.request_count() >= expected {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(ACCEPT_POLL_INTERVAL);
        }
    }

    /// Returns snapshots of all complete requests captured so far.
    #[allow(dead_code)]
    pub fn captured_requests(&self) -> Vec<CapturedRequest> {
        self.state
            .requests
            .lock()
            .expect("loopback request lock must not be poisoned")
            .clone()
    }

    /// Returns the first complete captured request, if any.
    pub fn first_request(&self) -> Option<CapturedRequest> {
        self.state
            .requests
            .lock()
            .expect("loopback request lock must not be poisoned")
            .first()
            .cloned()
    }

    /// Returns a listener or request-capture failure, if one occurred.
    pub fn failure(&self) -> Option<String> {
        self.state
            .failure
            .lock()
            .expect("loopback failure lock must not be poisoned")
            .clone()
    }

    /// Requests shutdown and waits for the worker to exit.
    #[allow(dead_code)]
    pub fn shutdown(mut self) -> io::Result<()> {
        self.request_shutdown();
        self.join_worker()
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    fn join_worker(&mut self) -> io::Result<()> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker
            .join()
            .map_err(|_| io::Error::other("loopback worker panicked while shutting down"))
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.request_shutdown();
        let _ = self.join_worker();
    }
}

struct ServerState {
    response: ResponsePlan,
    request_count: AtomicUsize,
    requests: Mutex<Vec<CapturedRequest>>,
    failure: Mutex<Option<String>>,
}

impl ServerState {
    fn new(response: ResponsePlan) -> Self {
        Self {
            response,
            request_count: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
        }
    }

    fn record_failure(&self, error: io::Error) {
        let mut failure = self
            .failure
            .lock()
            .expect("loopback failure lock must not be poisoned");
        if failure.is_none() {
            *failure = Some(error.to_string());
        }
    }
}

fn serve(listener: TcpListener, state: Arc<ServerState>, shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => match CapturedRequest::read_from(&mut stream) {
                Ok(request) => {
                    state.request_count.fetch_add(1, Ordering::SeqCst);
                    state
                        .requests
                        .lock()
                        .expect("loopback request lock must not be poisoned")
                        .push(request);
                    let _ = state.response.write_to(&mut stream, &shutdown);
                }
                Err(error) => state.record_failure(error),
            },
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(error) => {
                state.record_failure(error);
                return;
            }
        }
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn parse_request_headers(headers: &str) -> io::Result<ParsedRequestHeaders> {
    let mut lines = headers.split("\r\n");
    let request_line = lines.next().ok_or_else(|| {
        io::Error::new(ErrorKind::InvalidData, "loopback request line is missing")
    })?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidData, "loopback request method is missing")
        })?
        .to_owned();
    let target = request_parts
        .next()
        .ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidData, "loopback request target is missing")
        })?
        .to_owned();
    let version = request_parts
        .next()
        .ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidData,
                "loopback request version is missing",
            )
        })?
        .to_owned();
    if request_parts.next().is_some() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "loopback request line has too many fields",
        ));
    }

    let mut parsed_headers = Vec::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidData,
                "loopback request header is malformed",
            )
        })?;
        parsed_headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    Ok((method, target, version, parsed_headers))
}

fn content_length(headers: &[(String, String)]) -> io::Result<usize> {
    let Some((_, value)) = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
    else {
        return Ok(0);
    };
    value.parse::<usize>().map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidData,
            "loopback request content length is not an unsigned integer",
        )
    })
}

fn wait_without_shutdown(delay: Duration, shutdown: &AtomicBool) -> bool {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }
        thread::sleep(ACCEPT_POLL_INTERVAL);
    }
    !shutdown.load(Ordering::SeqCst)
}
