use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Instant;

use serde_json::{Value, json};

use super::{MAX_REQUEST_BYTES, REQUEST_HEADER_TIMEOUT};
use crate::export::csv_safe_cell;

pub(super) struct HttpRequest {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) params: HashMap<String, String>,
    /// Bearer token from the `Authorization` header, if any.
    pub(super) auth_token: Option<String>,
}

pub(super) struct HttpResponse {
    pub(super) status: u16,
    status_text: &'static str,
    content_type: &'static str,
    extra_headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    pub(super) fn new(
        status: u16,
        status_text: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            status_text,
            content_type,
            extra_headers: Vec::new(),
            body,
        }
    }

    pub(super) fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.extra_headers.push((name, value.into()));
        self
    }

    fn into_bytes(self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n\
             Content-Type: {}\r\n\
             Content-Length: {}\r\n\
             Cache-Control: no-store\r\n\
             Connection: close\r\n\
             X-Content-Type-Options: nosniff\r\n\
             Referrer-Policy: no-referrer\r\n",
            self.status,
            self.status_text,
            self.content_type,
            self.body.len()
        );
        for (name, value) in self.extra_headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(&value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        let mut out = Vec::with_capacity(head.len() + self.body.len());
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&self.body);
        out
    }

    pub(super) fn write(self, stream: &mut TcpStream) -> std::io::Result<()> {
        stream.write_all(&self.into_bytes())
    }
}

/// Reads the request head (request line + headers) up to the blank line. Only
/// newly read bytes are scanned for the terminator, so a large body or a flood
/// cannot trigger a quadratic rescan. The body is intentionally not read.
pub(super) fn read_head(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = [0u8; 4096];
    let mut bytes: Vec<u8> = Vec::new();
    let started = Instant::now();
    let mut complete = false;
    loop {
        let elapsed = started.elapsed();
        if elapsed >= REQUEST_HEADER_TIMEOUT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "HTTP request header timeout",
            ));
        }
        stream.set_read_timeout(Some(REQUEST_HEADER_TIMEOUT - elapsed))?;
        let read = stream.read(&mut buf)?;
        if read == 0 {
            break;
        }
        let scan_from = bytes.len().saturating_sub(3);
        bytes.extend_from_slice(&buf[..read]);
        if bytes[scan_from..].windows(4).any(|w| w == b"\r\n\r\n") {
            complete = true;
            break;
        }
        if bytes.len() >= MAX_REQUEST_BYTES {
            break;
        }
    }
    if !complete {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Incomplete HTTP request head",
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) fn parse_request(raw: &str) -> Result<HttpRequest, String> {
    let mut lines = raw.lines();
    let request_line = lines.next().ok_or("Empty HTTP request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("Missing HTTP method")?.to_string();
    let target = parts.next().ok_or("Missing HTTP target")?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let mut auth_token = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("authorization")
            && let Some(token) = value.trim().strip_prefix("Bearer ")
        {
            auth_token = Some(token.trim().to_string());
        }
    }

    Ok(HttpRequest {
        method,
        path: percent_decode(path),
        params: parse_query(query),
        auth_token,
    })
}

/// Length-checked, content-constant-time comparison so a network observer cannot
/// recover the token byte by byte through timing.
pub(super) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub(super) fn serve_static(content_type: &'static str, text: &str) -> HttpResponse {
    HttpResponse::new(200, "OK", content_type, text.as_bytes().to_vec())
}

pub(super) fn json_ok(value: Value) -> HttpResponse {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
    HttpResponse::new(200, "OK", "application/json; charset=utf-8", body)
}

pub(super) fn error_json(status: u16, status_text: &'static str, message: &str) -> HttpResponse {
    let body = serde_json::to_vec(&json!({ "ok": false, "error": message })).unwrap_or_default();
    HttpResponse::new(status, status_text, "application/json; charset=utf-8", body)
}

/// Wraps a handler result, turning an error string into a 500 JSON response.
pub(super) fn respond(result: Result<Value, String>) -> HttpResponse {
    match result {
        Ok(value) => json_ok(value),
        Err(err) => error_json(500, "Internal Server Error", &err),
    }
}

pub(super) fn parse_query(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key), percent_decode(value));
    }
    params
}

pub(super) fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
            {
                (Some(a), Some(b)) => {
                    out.push((a << 4) | b);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Escapes a cell for `;`-separated CSV, neutralizing spreadsheet formula cells.
pub(super) fn csv_cell(value: &str) -> String {
    let safe = csv_safe_cell(value);
    let value = safe.as_ref();
    if value.contains(';') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
