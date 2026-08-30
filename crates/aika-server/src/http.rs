//! Minimal, lenient HTTP server.
//!
//! The Aika client is from 2008 and talks to whatever Indy
//! (`TIdHTTPServer`) accepted: HTTP/1.0 without `Host`, line endings of
//! bare `\n`, an `application/x-www-form-urlencoded` body with no type.
//! Modern servers answer 400 to much of that, and a 400 here turns into a
//! login screen that hangs without saying why. Hence our own parser: it
//! accepts whatever it can make sense of and logs the raw bytes of what it
//! cannot, which is what you want on first contact with the client.

use std::net::IpAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Request size ceiling, so a bad connection cannot grow without bound.
const MAX_REQUEST: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub params: Params,
    pub remote: IpAddr,
}

#[derive(Debug, Clone, Default)]
pub struct Params(Vec<(String, String)>);

impl Params {
    /// Look up by name, case-insensitively.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Look up by position. The Delphi server reads `Params[0]`/`Params[1]`
    /// blindly, so clients sending unexpected names still work.
    pub fn nth(&self, index: usize) -> Option<&str> {
        self.0.get(index).map(|(_, v)| v.as_str())
    }

    /// Value by name, falling back to position when the name is absent.
    pub fn get_or_nth(&self, name: &str, index: usize) -> Option<&str> {
        self.get(name).or_else(|| self.nth(index))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn parse(input: &str) -> Self {
        let mut out = Vec::new();
        for pair in input.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            out.push((percent_decode(key), percent_decode(value)));
        }
        Params(out)
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

impl Response {
    pub fn text(body: impl Into<String>) -> Self {
        Self { status: 200, content_type: "text/plain; charset=utf-8", body: body.into() }
    }

    pub fn json(body: impl Into<String>) -> Self {
        Self { status: 200, content_type: "application/json", body: body.into() }
    }

    pub fn status(status: u16, body: impl Into<String>) -> Self {
        Self { status, content_type: "text/plain; charset=utf-8", body: body.into() }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            _ => "OK",
        };
        let head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.status,
            reason,
            self.content_type,
            self.body.as_bytes().len()
        );
        let mut out = head.into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        out
    }
}

#[derive(Debug)]
pub enum ParseError {
    /// The connection closed before a whole request arrived.
    Incomplete,
    Malformed(&'static str),
    TooLarge,
}

/// Reads a request from the socket. Returns `Ok(None)` when the client only
/// opened and closed the connection without sending anything (the launcher
/// does this to probe whether the server is up).
pub async fn read_request(
    stream: &mut TcpStream,
    remote: IpAddr,
) -> Result<Option<Request>, ParseError> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];

    // Headers: up to the first blank line.
    let head_end = loop {
        if let Some(pos) = find_head_end(&buf) {
            break pos;
        }
        if buf.len() > MAX_REQUEST {
            return Err(ParseError::TooLarge);
        }
        let n = stream.read(&mut chunk).await.map_err(|_| ParseError::Incomplete)?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(ParseError::Incomplete);
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end.0]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().ok_or(ParseError::Malformed("empty request line"))?;

    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(ParseError::Malformed("no method"))?.to_ascii_uppercase();
    let target = parts.next().unwrap_or("/");

    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

    if content_length > MAX_REQUEST {
        return Err(ParseError::TooLarge);
    }

    // Body, if any.
    let body_start = head_end.1;
    let mut body = buf[body_start..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await.map_err(|_| ParseError::Incomplete)?;
        if n == 0 {
            break; // truncated body: carry on with what arrived
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length.min(body.len()));

    // Old clients may send an absolute target (`http://host/path`).
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let path = strip_absolute_url(path).to_string();

    let mut params = Params::parse(query.unwrap_or(""));
    let body_text = String::from_utf8_lossy(&body);
    params.0.extend(Params::parse(&body_text).0);

    Ok(Some(Request { method, path, params, remote }))
}

pub async fn write_response(stream: &mut TcpStream, response: &Response) -> std::io::Result<()> {
    stream.write_all(&response.to_bytes()).await?;
    stream.flush().await
}

/// Where the headers end: `(end_of_text, start_of_body)`.
/// Accepts both `\r\n\r\n` and `\n\n`.
fn find_head_end(buf: &[u8]) -> Option<(usize, usize)> {
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| (p, p + 4));
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| (p, p + 2));
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn strip_absolute_url(target: &str) -> &str {
    for prefix in ["http://", "https://"] {
        if let Some(rest) = target.strip_prefix(prefix) {
            return match rest.find('/') {
                Some(slash) => &rest[slash..],
                None => "/",
            };
        }
    }
    target
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    // The client speaks latin-1; decode byte by byte so nothing is lost.
    out.iter().map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_form_params() {
        let params = Params::parse("id=admin&pw=1234");
        assert_eq!(params.get("id"), Some("admin"));
        assert_eq!(params.get("PW"), Some("1234"));
        assert_eq!(params.nth(0), Some("admin"));
        assert_eq!(params.get_or_nth("inexistente", 1), Some("1234"));
    }

    #[test]
    fn decodes_percent_and_plus() {
        let params = Params::parse("id=jo%C3%A3o+silva&pw=a%2Bb");
        assert_eq!(params.get("pw"), Some("a+b"));
        assert!(params.get("id").unwrap().contains(" silva"));
    }

    #[test]
    fn finds_head_end_with_both_terminators() {
        assert_eq!(find_head_end(b"GET / HTTP/1.0\r\n\r\nbody"), Some((14, 18)));
        assert_eq!(find_head_end(b"GET / HTTP/1.0\n\nbody"), Some((14, 16)));
        assert_eq!(find_head_end(b"GET / HTTP/1.0\r\n"), None);
    }

    #[test]
    fn strips_absolute_request_target() {
        assert_eq!(strip_absolute_url("http://host:8090/member/x.asp"), "/member/x.asp");
        assert_eq!(strip_absolute_url("/member/x.asp"), "/member/x.asp");
        assert_eq!(strip_absolute_url("http://host"), "/");
    }

    #[test]
    fn serializes_response() {
        let bytes = Response::text("CNT 1 0 0 0").to_bytes();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 11\r\n"));
        assert!(text.ends_with("\r\n\r\nCNT 1 0 0 0"));
    }
}
