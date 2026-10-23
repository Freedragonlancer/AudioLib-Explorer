// crates/audiolib-remote/src/test_server.rs
//! Minimal blocking HTTP/1.1 server for unit tests; binds 127.0.0.1 only.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

/// This build's compatibility catalog as JSON (a marker peer of the same release).
pub(crate) fn marker_catalog() -> serde_json::Value {
    serde_json::to_value(audiolib_core::compatibility::compatibility_manifest()).unwrap()
}

/// `/health` of a marker peer of this release.
pub(crate) fn marker_health() -> TestResponse {
    TestResponse::json(
        200,
        &serde_json::json!({"status":"ok","version":env!("CARGO_PKG_VERSION")}).to_string(),
    )
}

/// `/capabilities` of a marker peer with `extra` fields (an object) merged in.
pub(crate) fn marker_capabilities(extra: serde_json::Value) -> TestResponse {
    let mut body = extra;
    if !body.is_object() {
        body = serde_json::json!({});
    }
    body["compatibility"] = marker_catalog();
    TestResponse::json(200, &body.to_string())
}

/// Marker-peer handshake for tests focused on unrelated API operations.
pub(crate) fn marker_handshake(path: &str) -> Option<TestResponse> {
    match path {
        "/api/v1/health" => Some(marker_health()),
        "/api/v1/capabilities" => Some(marker_capabilities(serde_json::json!({}))),
        _ => None,
    }
}

pub(crate) struct TestRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    /// Request body (read by `Content-Length`).
    pub body: Vec<u8>,
}

impl TestRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub(crate) struct TestResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
    /// Write the body in pieces of this size, sleeping in between.
    pub chunk: Option<(usize, Duration)>,
    /// Close the connection after this many body bytes (Content-Length stays complete).
    pub cut_after: Option<usize>,
    /// Wait this long before sending anything (a NAS disk spinning up).
    pub delay: Option<Duration>,
    /// Connection-close framing without a known transfer size.
    pub omit_content_length: bool,
}

impl TestResponse {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: body.as_bytes().to_vec(),
            headers: Vec::new(),
            chunk: None,
            cut_after: None,
            delay: None,
            omit_content_length: false,
        }
    }
    pub fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: "application/octet-stream",
            body,
            headers: Vec::new(),
            chunk: None,
            cut_after: None,
            delay: None,
            omit_content_length: false,
        }
    }
    pub fn slow(mut self, size: usize, delay: Duration) -> Self {
        self.chunk = Some((size.max(1), delay));
        self
    }
    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
    pub fn cut_after(mut self, bytes: usize) -> Self {
        self.cut_after = Some(bytes);
        self
    }
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn without_content_length(mut self) -> Self {
        self.omit_content_length = true;
        self
    }
}

/// Serves each connection on its own thread until the test process exits.
pub(crate) struct TestServer {
    pub url: String,
}

impl TestServer {
    pub fn start(handler: impl Fn(&TestRequest) -> TestResponse + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handler = Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let handler = handler.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_owned();
                    let path = parts.next().unwrap_or_default().to_owned();
                    let mut headers = Vec::new();
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 {
                            break;
                        }
                        let header = header.trim_end();
                        if header.is_empty() {
                            break;
                        }
                        if let Some((key, value)) = header.split_once(':') {
                            headers.push((key.trim().to_owned(), value.trim().to_owned()));
                        }
                    }
                    let length = headers
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    let mut body = vec![0u8; length];
                    if reader.read_exact(&mut body).is_err() {
                        return;
                    }
                    let request = TestRequest {
                        method,
                        path,
                        headers,
                        body,
                    };
                    let response = handler(&request);
                    if let Some(delay) = response.delay {
                        std::thread::sleep(delay);
                    }
                    let extra: String = response
                        .headers
                        .iter()
                        .map(|(name, value)| format!("{name}: {value}\r\n"))
                        .collect();
                    let content_length = if response.omit_content_length {
                        String::new()
                    } else {
                        format!("Content-Length: {}\r\n", response.body.len())
                    };
                    let head = format!(
                        "HTTP/1.1 {} Test\r\nContent-Type: {}\r\n{content_length}{extra}Connection: close\r\n\r\n",
                        response.status, response.content_type
                    );
                    let _ = stream.write_all(head.as_bytes());
                    if request.method != "HEAD" {
                        let end = response
                            .cut_after
                            .unwrap_or(usize::MAX)
                            .min(response.body.len());
                        let body = &response.body[..end];
                        match response.chunk {
                            Some((size, delay)) => {
                                for piece in body.chunks(size) {
                                    if stream.write_all(piece).is_err() || stream.flush().is_err() {
                                        break;
                                    }
                                    std::thread::sleep(delay);
                                }
                            }
                            None => {
                                let _ = stream.write_all(body);
                            }
                        }
                    }
                    let _ = stream.flush();
                });
            }
        });
        Self { url }
    }
}
