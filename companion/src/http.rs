//! A very small blocking HTTP/1.1 client.
//!
//! The OpenCode server is local, so we only need plain HTTP with no TLS and no
//! connection pooling. Keeping this hand-rolled avoids pulling a large async
//! stack into a binary that must build offline and on three platforms.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// A parsed base URL plus optional credentials and location scope.
#[derive(Debug, Clone)]
pub struct Client {
    host: String,
    port: u16,
    auth: Option<String>,
    /// Value for the `x-opencode-directory` header that scopes requests to a project.
    directory: Option<String>,
}

/// A fully-read HTTP response.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.body)
            .with_context(|| format!("decoding JSON response: {}", truncate(&self.text(), 400)))
    }

    /// Return `Ok(self)` when successful, otherwise an error carrying the body.
    pub fn error_for_status(self) -> Result<Self> {
        if self.is_success() {
            Ok(self)
        } else {
            bail!("HTTP {}: {}", self.status, truncate(&self.text(), 400))
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

impl Client {
    /// Build a client from a base URL such as `http://127.0.0.1:4096`.
    pub fn new(base_url: &str, password: Option<String>) -> Result<Self> {
        let rest = base_url
            .strip_prefix("http://")
            .or_else(|| base_url.strip_prefix("https://"))
            .unwrap_or(base_url);
        let rest = rest.trim_end_matches('/');
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>().with_context(|| format!("bad port in {base_url}"))?,
            ),
            None => (rest.to_string(), 80),
        };
        if host.is_empty() {
            bail!("could not parse host from base URL {base_url}");
        }
        Ok(Self {
            host,
            port,
            auth: password.map(|p| auth_header(&p)),
            directory: None,
        })
    }

    /// The `Authorization` header value used for the local service.
    pub fn auth(&self) -> Option<&str> {
        self.auth.as_deref()
    }

    /// Scope subsequent requests to a project directory.
    pub fn set_directory(&mut self, directory: Option<String>) {
        self.directory = directory;
    }

    /// The currently scoped directory, if any.
    pub fn directory(&self) -> Option<&str> {
        self.directory.as_deref()
    }

    fn connect(&self, timeout: Duration) -> Result<TcpStream> {
        let addr = format!("{}:{}", self.host, self.port);
        let stream = TcpStream::connect(&addr)
            .with_context(|| format!("connecting to OpenCode server at {addr}"))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true).ok();
        Ok(stream)
    }

    fn write_request(
        &self,
        stream: &mut TcpStream,
        method: &str,
        path: &str,
        accept: &str,
        body: Option<(&str, &[u8])>,
    ) -> Result<()> {
        let mut head = String::new();
        head.push_str(&format!("{method} {path} HTTP/1.1\r\n"));
        head.push_str(&format!("Host: {}:{}\r\n", self.host, self.port));
        head.push_str(&format!("Accept: {accept}\r\n"));
        head.push_str("Connection: close\r\n");
        head.push_str("User-Agent: ocw-companion/0.1\r\n");
        if let Some(auth) = &self.auth {
            head.push_str(&format!("Authorization: {auth}\r\n"));
        }
        if let Some(dir) = &self.directory {
            head.push_str(&format!("x-opencode-directory: {dir}\r\n"));
        }
        if let Some((content_type, bytes)) = body {
            head.push_str(&format!("Content-Type: {content_type}\r\n"));
            head.push_str(&format!("Content-Length: {}\r\n", bytes.len()));
        }
        head.push_str("\r\n");

        stream.write_all(head.as_bytes())?;
        if let Some((_, bytes)) = body {
            stream.write_all(bytes)?;
        }
        stream.flush()?;
        Ok(())
    }

    /// Perform a request and read the full response body.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        accept: &str,
        body: Option<(&str, &[u8])>,
        timeout: Duration,
    ) -> Result<Response> {
        let mut stream = self.connect(timeout)?;
        self.write_request(&mut stream, method, path, accept, body)?;

        let mut reader = BufReader::new(stream);
        let (status, headers) = read_headers(&mut reader)?;
        let body = read_body(&mut reader, &headers)?;
        Ok(Response { status, body })
    }

    /// `GET` a path and return the parsed response.
    pub fn get(&self, path: &str) -> Result<Response> {
        self.request("GET", path, "application/json", None, Duration::from_secs(30))
    }

    /// `GET` a path and parse the JSON body.
    pub fn get_json(&self, path: &str) -> Result<serde_json::Value> {
        self.get(path)?.error_for_status()?.json()
    }

    /// `DELETE` a path.
    pub fn delete(&self, path: &str) -> Result<Response> {
        self.request("DELETE", path, "application/json", None, Duration::from_secs(30))
    }

    /// `POST` a JSON body with the default timeout.
    pub fn post_json(&self, path: &str, value: &serde_json::Value) -> Result<Response> {
        self.post_json_timeout(path, value, Duration::from_secs(30))
    }

    /// `POST` a JSON body with an explicit timeout.
    pub fn post_json_timeout(
        &self,
        path: &str,
        value: &serde_json::Value,
        timeout: Duration,
    ) -> Result<Response> {
        let bytes = serde_json::to_vec(value)?;
        self.request(
            "POST",
            path,
            "application/json",
            Some(("application/json", &bytes)),
            timeout,
        )
    }

    /// Open a streaming response (used for the server-sent event feed).
    pub fn get_stream(&self, path: &str, timeout: Duration) -> Result<Box<dyn BufRead>> {
        let mut stream = self.connect(timeout)?;
        self.write_request(&mut stream, "GET", path, "text/event-stream", None)?;
        let mut reader = BufReader::new(stream);
        let (status, headers) = read_headers(&mut reader)?;
        if !(200..300).contains(&status) {
            bail!("event stream returned HTTP {status}");
        }
        if header_contains(&headers, "transfer-encoding", "chunked") {
            Ok(Box::new(BufReader::new(ChunkedReader::new(reader))))
        } else {
            Ok(Box::new(reader))
        }
    }
}

/// Build the `Authorization` header value for the local service password.
///
/// The OpenCode background service uses HTTP Basic auth with the literal
/// username `opencode` and the service password.
fn auth_header(password: &str) -> String {
    format!(
        "Basic {}",
        base64_encode(format!("opencode:{password}").as_bytes())
    )
}

/// Minimal standard-base64 encoder (avoids a runtime dependency for one call).
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn read_headers<R: BufRead>(reader: &mut R) -> Result<(u16, Vec<(String, String)>)> {
    let mut status_line = String::new();
    if reader.read_line(&mut status_line)? == 0 {
        bail!("empty response from server");
    }
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .with_context(|| format!("malformed status line: {status_line:?}"))?;

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    Ok((status, headers))
}

fn header_contains(headers: &[(String, String)], key: &str, needle: &str) -> bool {
    headers
        .iter()
        .any(|(k, v)| k == key && v.to_ascii_lowercase().contains(needle))
}

fn header_value<'a>(headers: &'a [(String, String)], key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn read_body<R: BufRead>(reader: &mut R, headers: &[(String, String)]) -> Result<Vec<u8>> {
    if header_contains(headers, "transfer-encoding", "chunked") {
        let mut out = Vec::new();
        ChunkedReader::new(reader).read_to_end(&mut out)?;
        return Ok(out);
    }
    if let Some(len) = header_value(headers, "content-length").and_then(|v| v.parse::<usize>().ok()) {
        let mut out = vec![0u8; len];
        reader.read_exact(&mut out)?;
        return Ok(out);
    }
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

/// Decodes an HTTP/1.1 `Transfer-Encoding: chunked` body.
pub struct ChunkedReader<R: BufRead> {
    inner: R,
    remaining: usize,
    done: bool,
}

impl<R: BufRead> ChunkedReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            remaining: 0,
            done: false,
        }
    }

    fn next_chunk_size(&mut self) -> std::io::Result<usize> {
        let mut line = String::new();
        self.inner.read_line(&mut line)?;
        let size = line
            .trim()
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        usize::from_str_radix(size, 16).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid chunk size {size:?}"),
            )
        })
    }
}

impl<R: BufRead> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.remaining == 0 {
            let size = self.next_chunk_size()?;
            if size == 0 {
                self.done = true;
                return Ok(0);
            }
            self.remaining = size;
        }
        let want = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            self.done = true;
            return Ok(0);
        }
        self.remaining -= n;
        if self.remaining == 0 {
            let mut crlf = [0u8; 2];
            let _ = self.inner.read_exact(&mut crlf);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_base_url() {
        let c = Client::new("http://127.0.0.1:4096", None).unwrap();
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.port, 4096);

        let c = Client::new("http://localhost:8080/", Some("pw".into())).unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.auth(), Some("Basic b3BlbmNvZGU6cHc="));
    }

    #[test]
    fn base64_matches_reference_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn decodes_chunked_bodies() {
        let raw = "5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let mut reader = BufReader::new(Cursor::new(raw.as_bytes().to_vec()));
        let mut out = String::new();
        ChunkedReader::new(&mut reader).read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn reads_headers() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Y: z\r\n\r\n";
        let mut reader = BufReader::new(Cursor::new(raw.as_bytes().to_vec()));
        let (status, headers) = read_headers(&mut reader).unwrap();
        assert_eq!(status, 200);
        assert_eq!(header_value(&headers, "content-type"), Some("application/json"));
    }
}
