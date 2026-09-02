//! A minimal HTTP/1.1 request reader and response writer over
//! `std::net::TcpStream`. Scoped to what this server actually needs to
//! speak: request line + headers + a `Content-Length`-framed body in (JSON
//! POST bodies from a trusted internal client, not arbitrary web traffic),
//! and either a single buffered JSON response or a `Transfer-Encoding:
//! chunked` streamed sequence of NDJSON lines out — real chunked framing,
//! not a connection-close shortcut, so any standards-compliant HTTP client
//! (curl, Node's fetch, JARVIS's own HTTP calls) can consume it correctly.
//!
//! No keep-alive pipelining, no compression, no TLS — this serves one
//! request per connection to a local, trusted caller.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug)]
pub struct HttpError(pub String);
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP parse error: {}", self.0)
    }
}
impl std::error::Error for HttpError {}
impl From<io::Error> for HttpError {
    fn from(e: io::Error) -> Self {
        HttpError(e.to_string())
    }
}

pub fn read_request(stream: &TcpStream) -> Result<Request, HttpError> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let request_line = request_line.trim_end();
    if request_line.is_empty() {
        return Err(HttpError("empty request line (client closed connection)".into()));
    }
    let mut parts = request_line.split(' ');
    let method = parts.next().ok_or_else(|| HttpError("missing method".into()))?.to_string();
    let path = parts.next().ok_or_else(|| HttpError("missing path".into()))?.to_string();
    // HTTP version (3rd token) intentionally ignored — this server only
    // ever emits HTTP/1.1 responses and doesn't negotiate versions.

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    Ok(Request { method, path, headers, body })
}

/// Writes a single, complete, non-streamed JSON response and returns —
/// callers must not write anything else to `stream` afterward.
pub fn write_json_response(mut stream: &TcpStream, status: u16, body: &str) -> io::Result<()> {
    let status_text = status_text(status);
    write!(
        stream,
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Unknown",
    }
}

/// A `Transfer-Encoding: chunked` NDJSON stream — one JSON object per line,
/// each line wrapped in its own HTTP chunk so the framing is real chunked
/// transfer coding, not just "write bytes and hope the client buffers
/// them". Headers are written once, on `start()`; call `write_line()` per
/// NDJSON object, then `finish()` to emit the terminating zero-length chunk.
pub struct ChunkedJsonStream<'a> {
    stream: &'a TcpStream,
}

impl<'a> ChunkedJsonStream<'a> {
    pub fn start(stream: &'a TcpStream) -> io::Result<Self> {
        let mut s = stream;
        write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")?;
        s.flush()?;
        Ok(ChunkedJsonStream { stream })
    }

    /// Writes one NDJSON line (a trailing `\n` is added) as its own chunk.
    pub fn write_line(&mut self, json_line: &str) -> io::Result<()> {
        let mut s = self.stream;
        let payload_len = json_line.len() + 1; // +1 for the trailing '\n'
        write!(s, "{payload_len:x}\r\n{json_line}\n\r\n")?;
        s.flush()
    }

    pub fn finish(self) -> io::Result<()> {
        let mut s = self.stream;
        write!(s, "0\r\n\r\n")?;
        s.flush()
    }
}
