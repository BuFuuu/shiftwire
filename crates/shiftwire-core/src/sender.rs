//! Sends a raw, hand-written HTTP request and records it like any other flow.
//! Used by the Repeater and by MCP's `send_request`.
//!
//! The bytes go out exactly as written: header case, order and duplicates are
//! kept (pentesting needs that; an HTTP library would normalize them). Only
//! `Content-Length` is corrected (unless `Transfer-Encoding` is set) and a
//! missing `Host` is filled in. Each send uses a fresh connection.

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use hyper::{Method, Uri, Version};
use rustls::pki_types::ServerName;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::model::{Flow, header};
use crate::proxy::capture::BodyCapture;
use crate::proxy::tls;
use crate::{App, now_ms};

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HEAD: usize = 64 * 1024;

/// Where a request is sent; may differ from its `Host` header, on purpose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl Target {
    pub fn validate(&self) -> io::Result<()> {
        let host_ok = !self.host.is_empty() && !self.host.contains(['/', ' ', '?', '#', '@']);
        if !matches!(self.scheme.as_str(), "http" | "https") || !host_ok || self.port == 0 {
            return Err(invalid(format!(
                "invalid target {}://{}:{}",
                self.scheme, self.host, self.port
            )));
        }
        Ok(())
    }

    /// `host[:port]`, omitting the scheme's default port; IPv6 in brackets.
    fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default = if self.scheme == "https" { 443 } else { 80 };
        if self.port == default {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

/// A request as written in the editor.
#[derive(Debug, PartialEq)]
pub(crate) struct RawRequest {
    pub method: Method,
    /// Origin-form: `/path?query` (or `*`).
    pub path: String,
    pub version: Version,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RawRequest {
    fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// Make `Content-Length` match the body, keeping its position.
    fn fix_content_length(&mut self) {
        if self.has_header("transfer-encoding") {
            return; // hand-crafted framing: leave it alone
        }
        let len = self.body.len().to_string();
        let mut seen = false;
        self.headers.retain_mut(|(name, value)| {
            if !name.eq_ignore_ascii_case("content-length") {
                return true;
            }
            *value = len.clone();
            !std::mem::replace(&mut seen, true) // keep only the first
        });
        if !seen && !self.body.is_empty() {
            self.headers.push(("Content-Length".into(), len));
        }
    }

    /// Back to editor text (`\n` line endings).
    fn to_text(&self) -> String {
        let mut out = format!("{} {} {:?}\n", self.method, self.path, self.version);
        for (name, value) in &self.headers {
            out.push_str(&format!("{name}: {value}\n"));
        }
        out.push('\n');
        out.push_str(&String::from_utf8_lossy(&self.body));
        out
    }

    /// Request line and headers as written.
    fn head(&self) -> String {
        let mut out = format!("{} {} {:?}\r\n", self.method, self.path, self.version);
        for (name, value) in &self.headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out
    }
}

/// The next request when `flow` (sent as `raw` to `target`) is a redirect to follow,
/// with the rules browsers use: 303, and 301/302 after a POST, become a GET without
/// a body; 307/308 repeat method and body. `Host` follows the new location.
pub(crate) fn redirect(target: &Target, raw: &str, flow: &Flow) -> Option<(Target, String)> {
    let status = flow.status?;
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = header(&flow.resp_head, "location")?.trim();
    let (next, path) = resolve(target, &flow.path, location)?;
    let mut req = parse(raw).ok()?;
    let to_get = status == 303 || (matches!(status, 301 | 302) && req.method == Method::POST);
    if to_get {
        if req.method != Method::HEAD {
            req.method = Method::GET;
        }
        req.body.clear();
        req.headers.retain(|(n, _)| {
            !["content-length", "content-type", "transfer-encoding"]
                .iter()
                .any(|h| n.eq_ignore_ascii_case(h))
        });
    }
    req.path = path;
    let authority = next.authority();
    match req
        .headers
        .iter_mut()
        .find(|(n, _)| n.eq_ignore_ascii_case("host"))
    {
        Some((_, value)) => *value = authority,
        None => req.headers.insert(0, ("Host".into(), authority)),
    }
    Some((next, req.to_text()))
}

/// `Location` against the current target and path: absolute, `//host/...`,
/// `/path` or relative to the current directory.
fn resolve(target: &Target, current_path: &str, location: &str) -> Option<(Target, String)> {
    if location.starts_with("//") {
        return resolve(
            target,
            current_path,
            &format!("{}:{location}", target.scheme),
        );
    }
    if location.contains("://") {
        let uri: Uri = location.parse().ok()?;
        let scheme = uri.scheme_str()?.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return None;
        }
        let default = if scheme == "https" { 443 } else { 80 };
        let next = Target {
            host: uri.host()?.trim_matches(['[', ']']).to_string(),
            port: uri.port_u16().unwrap_or(default),
            scheme,
        };
        let path = uri.path_and_query().map_or("/", |p| p.as_str());
        let path = if path.is_empty() { "/" } else { path };
        return Some((next, path.to_string()));
    }
    let path = if location.starts_with('/') {
        location.to_string()
    } else {
        let dir = current_path.rsplit_once('/').map_or("", |(d, _)| d);
        format!("{dir}/{location}")
    };
    Some((target.clone(), path))
}

/// Parses editor text: request line, headers, blank line, body. `\n` or `\r\n`.
pub(crate) fn parse(text: &str) -> Result<RawRequest, String> {
    let text = text.trim_start_matches(['\r', '\n']);
    let (head, body) = match (text.find("\r\n\r\n"), text.find("\n\n")) {
        (Some(a), Some(b)) if b < a => (&text[..b], &text[b + 2..]),
        (Some(a), _) => (&text[..a], &text[a + 4..]),
        (None, Some(b)) => (&text[..b], &text[b + 2..]),
        (None, None) => (text.trim_end(), ""),
    };
    let mut lines = head.lines();
    let first = lines.next().ok_or("the request is empty")?;
    let mut parts = first.split_whitespace();
    let method = parts.next().ok_or("missing method")?;
    let method =
        Method::from_bytes(method.as_bytes()).map_err(|_| format!("invalid method {method:?}"))?;
    let target = parts.next().ok_or("missing path (e.g. GET / HTTP/1.1)")?;
    let version = match parts.next().unwrap_or("HTTP/1.1") {
        "HTTP/1.1" => Version::HTTP_11,
        "HTTP/1.0" => Version::HTTP_10,
        other => return Err(format!("unsupported version {other} (use HTTP/1.1)")),
    };
    let path = if target.starts_with("http://") || target.starts_with("https://") {
        let uri: Uri = target
            .parse()
            .map_err(|_| format!("invalid URL {target:?}"))?;
        uri.path_and_query().map_or("/", |p| p.as_str()).to_string()
    } else if target.starts_with('/') || target == "*" {
        target.to_string()
    } else {
        return Err(format!("the path must start with '/': {target:?}"));
    };
    let mut headers = Vec::new();
    for (n, line) in lines.enumerate() {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("line {}: expected 'Name: value', got {line:?}", n + 2))?;
        let name = name.trim();
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c.is_control()) {
            return Err(format!("line {}: invalid header name {name:?}", n + 2));
        }
        headers.push((name.to_string(), value.trim().to_string()));
    }
    Ok(RawRequest {
        method,
        path,
        version,
        headers,
        body: body.as_bytes().to_vec(),
    })
}

/// Checks that editor text parses as a request (used before preparing one).
pub(crate) fn check_raw(raw: &str) -> io::Result<()> {
    parse(raw).map(|_| ()).map_err(invalid)
}

/// Editor text for a captured request (origin-form request line, `\n` endings).
pub(crate) fn raw_from_flow(flow: &Flow) -> io::Result<String> {
    if flow.method == "CONNECT" {
        return Err(invalid("a tunnel has no request to repeat".into()));
    }
    let mut lines = flow.req_head.lines();
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let (method, target, version) = (
        parts.next().unwrap_or("GET"),
        parts.next().unwrap_or("/"),
        parts.next().unwrap_or("HTTP/1.1"),
    );
    let path = target
        .parse::<Uri>()
        .ok()
        .and_then(|u| u.path_and_query().map(|p| p.as_str().to_string()))
        .unwrap_or_else(|| target.to_string());
    let mut out = format!("{method} {path} {version}\n");
    for line in lines.filter(|l| !l.is_empty()) {
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&String::from_utf8_lossy(&flow.req_body));
    Ok(out)
}

/// Changes an agent asks for when repeating a captured request.
#[derive(Debug, Default, Deserialize)]
pub struct Edits {
    pub method: Option<String>,
    /// New request target, e.g. `/api/users/8?admin=1`.
    pub path: Option<String>,
    /// Replace (or add) these headers.
    #[serde(default)]
    pub set_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub remove_headers: Vec<String>,
    pub body: Option<String>,
}

pub(crate) fn apply_edits(raw: &str, e: &Edits) -> io::Result<String> {
    let mut r = parse(raw).map_err(invalid)?;
    if let Some(m) = &e.method {
        r.method = Method::from_bytes(m.trim().as_bytes())
            .map_err(|_| invalid(format!("invalid method {m:?}")))?;
    }
    if let Some(p) = &e.path {
        if !p.starts_with('/') {
            return Err(invalid(format!("path must start with '/': {p:?}")));
        }
        r.path = p.clone();
    }
    r.headers
        .retain(|(n, _)| !e.remove_headers.iter().any(|x| x.eq_ignore_ascii_case(n)));
    for (name, value) in &e.set_headers {
        match r
            .headers
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(h) => h.1 = value.clone(),
            None => r.headers.push((name.clone(), value.clone())),
        }
    }
    if let Some(body) = &e.body {
        r.body = body.clone().into_bytes();
    }
    Ok(r.to_text())
}

/// Sends `raw` to `target` and records the exchange. Network failures are
/// recorded on the flow (`error`), not returned as `Err`.
pub(crate) async fn send(
    app: &App,
    target: &Target,
    raw: &str,
    actor: &str,
    source: &str,
    blocked: Option<String>,
) -> io::Result<Flow> {
    target.validate()?;
    let mut req = parse(raw).map_err(invalid)?;
    if !req.has_header("host") {
        req.headers.insert(0, ("Host".into(), target.authority()));
    }
    req.fix_content_length();

    let limit = app.config().max_body_bytes;
    let mut sent = BodyCapture::new(limit);
    sent.push(&req.body);
    let (path, query) = match req.path.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (req.path.clone(), None),
    };
    let mut flow = Flow {
        id: crate::new_id(),
        seq: app.next_seq(),
        intent: app.current_intent(actor),
        ts_start: now_ms(),
        actor: actor.into(),
        source: source.into(),
        scheme: target.scheme.clone(),
        host: target.host.clone(),
        port: target.port,
        method: req.method.to_string(),
        path,
        query,
        http_version: format!("{:?}", req.version),
        req_head: req.head(),
        req_size: sent.total,
        req_trunc: sent.truncated(),
        req_body: sent.data,
        ..Flow::default()
    };

    // An agent's own sends obey the alert rules too, so it cannot get around a block.
    let mut blocked = blocked;
    if source == "mcp" {
        let mut outcome = crate::rules::Outcome::default();
        let headers: Vec<String> = req
            .headers
            .iter()
            .map(|(n, v)| format!("{n}: {v}"))
            .collect();
        app.compiled_rules()
            .check(&headers.join("\n"), &req.body, &mut outcome);
        for rule in &outcome.alerts {
            app.rule_alert(rule, &flow, outcome.blocked.as_ref() == Some(rule));
        }
        blocked = blocked.or_else(|| outcome.block_reason());
        flow.tags = outcome.tags;
    }

    if let Some(reason) = blocked {
        // Refused (paused, out of scope, or an alert rule blocks it): recorded so the operator sees the attempt.
        flow.error = Some(reason);
        flow.ts_end = now_ms();
        app.record(flow.clone());
        return Ok(flow);
    }

    let mut bytes = req.head().into_bytes();
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(&req.body);
    let head_request = req.method == Method::HEAD;
    let outcome = tokio::time::timeout(TIMEOUT, async {
        let tcp = TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|e| format!("shiftWire could not reach {}: {e}", target.authority()))?;
        if target.scheme == "https" {
            let config = tls::upstream_config(app.config().verify_upstream_tls, app.crypto());
            let name = ServerName::try_from(target.host.clone())
                .map_err(|e| format!("invalid host: {e}"))?;
            let stream = TlsConnector::from(Arc::new(config))
                .connect(name, tcp)
                .await
                .map_err(|e| format!("TLS with {} failed: {e}", target.authority()))?;
            exchange(stream, &bytes, head_request, limit).await
        } else {
            exchange(tcp, &bytes, head_request, limit).await
        }
    })
    .await;

    match outcome {
        Ok(Ok(resp)) => {
            flow.status = Some(resp.status);
            flow.mime = header(&resp.head, "content-type")
                .map(|v| v.split(';').next().unwrap_or(v).trim().to_string());
            flow.resp_head = resp.head;
            (flow.resp_size, flow.resp_trunc, flow.resp_body) =
                (resp.body.total, resp.body.truncated(), resp.body.data);
            flow.error = resp.error;
        }
        Ok(Err(e)) => flow.error = Some(e),
        Err(_) => {
            flow.error = Some(format!(
                "no complete response within {} s",
                TIMEOUT.as_secs()
            ))
        }
    }
    flow.ts_end = now_ms();
    app.record(flow.clone());
    Ok(flow)
}

struct Received {
    head: String,
    status: u16,
    body: BodyCapture,
    /// The body broke off (head and partial body are still kept).
    error: Option<String>,
}

/// Writes the request, then reads one response: skips interim 1xx responses and
/// handles `Content-Length`, chunked and read-until-close bodies.
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    request: &[u8],
    head_request: bool,
    limit: usize,
) -> Result<Received, String> {
    stream
        .write_all(request)
        .await
        .map_err(|e| format!("sending failed: {e}"))?;
    stream
        .flush()
        .await
        .map_err(|e| format!("sending failed: {e}"))?;
    let mut reader = BufReader::new(stream);
    loop {
        let raw_head = read_head(&mut reader).await?;
        let mut headers = [httparse::EMPTY_HEADER; 128];
        let mut parsed = httparse::Response::new(&mut headers);
        if !matches!(parsed.parse(&raw_head), Ok(httparse::Status::Complete(_))) {
            return Err("the server sent a malformed response".into());
        }
        let status = parsed.code.unwrap_or(0);
        if (100..200).contains(&status) && status != 101 {
            continue; // e.g. 100 Continue: the real response follows
        }
        let value = |name: &str| {
            parsed
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .map(|h| String::from_utf8_lossy(h.value).trim().to_ascii_lowercase())
        };
        let chunked = value("transfer-encoding").is_some_and(|v| v.contains("chunked"));
        let length = value("content-length").and_then(|v| v.parse::<u64>().ok());
        let head = String::from_utf8_lossy(&raw_head).trim_end().to_string() + "\r\n";

        let mut body = BodyCapture::new(limit);
        let read = if head_request || matches!(status, 101 | 204 | 304) {
            Ok(())
        } else if chunked {
            read_chunked(&mut reader, &mut body).await
        } else if let Some(n) = length {
            copy_body(&mut reader.take(n), &mut body, Some(n)).await
        } else {
            copy_body(&mut reader, &mut body, None).await
        };
        return Ok(Received {
            head,
            status,
            body,
            error: read.err(),
        });
    }
}

async fn read_head<R: AsyncBufReadExt + Unpin>(r: &mut R) -> Result<Vec<u8>, String> {
    let mut head = Vec::new();
    loop {
        let n = r
            .read_until(b'\n', &mut head)
            .await
            .map_err(|e| format!("reading the response failed: {e}"))?;
        if n == 0 {
            return Err("the connection closed before a response arrived".into());
        }
        if head.ends_with(b"\r\n\r\n") || head.ends_with(b"\n\n") {
            return Ok(head);
        }
        if head.len() > MAX_HEAD {
            return Err("the response head is larger than 64 KB".into());
        }
    }
}

/// Copies a body into `capture`; `expected` is the announced length, if any.
async fn copy_body<R: AsyncRead + Unpin>(
    r: &mut R,
    capture: &mut BodyCapture,
    expected: Option<u64>,
) -> Result<(), String> {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = r
            .read(&mut buf)
            .await
            .map_err(|e| format!("response body: {e}"))?;
        if n == 0 {
            break;
        }
        capture.push(&buf[..n]);
    }
    match expected {
        Some(n) if capture.total < n => Err(format!(
            "response body ended after {} of {n} bytes",
            capture.total
        )),
        _ => Ok(()),
    }
}

/// Decodes `Transfer-Encoding: chunked` (bodies are stored de-chunked, like proxied ones).
async fn read_chunked<R: AsyncBufReadExt + Unpin>(
    r: &mut R,
    capture: &mut BodyCapture,
) -> Result<(), String> {
    let mut line = String::new();
    loop {
        line.clear();
        r.read_line(&mut line)
            .await
            .map_err(|e| format!("chunked body: {e}"))?;
        let size = line.split(';').next().unwrap_or_default().trim();
        let size = u64::from_str_radix(size, 16).map_err(|_| format!("bad chunk size {size:?}"))?;
        if size == 0 {
            // Trailers until an empty line.
            loop {
                line.clear();
                if r.read_line(&mut line).await.map_err(|e| e.to_string())? == 0
                    || line.trim().is_empty()
                {
                    return Ok(());
                }
            }
        }
        copy_body(&mut r.take(size), capture, Some(capture.total + size)).await?;
        line.clear();
        r.read_line(&mut line)
            .await
            .map_err(|e| format!("chunked body: {e}"))?; // CRLF after the chunk
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_editor_text_with_either_line_ending() {
        for text in [
            "POST /login?x=1 HTTP/1.1\nHost: a.test\nX-A: 1\n\nuser=a",
            "POST /login?x=1 HTTP/1.1\r\nHost: a.test\r\nX-A: 1\r\n\r\nuser=a",
        ] {
            let r = parse(text).unwrap();
            assert_eq!(r.method, Method::POST);
            assert_eq!(r.path, "/login?x=1");
            assert_eq!(
                r.headers,
                vec![("Host".into(), "a.test".into()), ("X-A".into(), "1".into())]
            );
            assert_eq!(r.body, b"user=a");
        }
        assert_eq!(
            parse("GET https://a.test/p?q HTTP/1.1").unwrap().path,
            "/p?q"
        );
        assert_eq!(parse("\n\nDELETE /x").unwrap().version, Version::HTTP_11);
    }

    #[test]
    fn parse_errors_say_what_is_wrong() {
        assert!(parse("").unwrap_err().contains("empty"));
        assert!(parse("GET").unwrap_err().contains("missing path"));
        assert!(
            parse("GET x HTTP/1.1")
                .unwrap_err()
                .contains("must start with '/'")
        );
        assert!(
            parse("GET / HTTP/2")
                .unwrap_err()
                .contains("unsupported version")
        );
        assert!(
            parse("GET / HTTP/1.1\nno colon here")
                .unwrap_err()
                .contains("line 2")
        );
        assert!(
            parse("GET / HTTP/1.1\nbad name: x")
                .unwrap_err()
                .contains("invalid header name")
        );
    }

    #[test]
    fn content_length_follows_the_body() {
        let mut r =
            parse("POST / HTTP/1.1\nContent-Length: 999\nX: y\nContent-length: 5\n\nabc").unwrap();
        r.fix_content_length();
        assert_eq!(
            r.headers,
            vec![
                ("Content-Length".into(), "3".into()),
                ("X".into(), "y".into())
            ]
        );

        let mut added = parse("POST / HTTP/1.1\n\nabcd").unwrap();
        added.fix_content_length();
        assert!(
            added
                .headers
                .contains(&("Content-Length".into(), "4".into()))
        );

        let mut chunked =
            parse("POST / HTTP/1.1\nTransfer-Encoding: chunked\nContent-Length: 1\n\n0\r\n\r\n")
                .unwrap();
        chunked.fix_content_length();
        assert!(
            chunked
                .headers
                .contains(&("Content-Length".into(), "1".into())),
            "crafted framing is kept"
        );
    }

    #[test]
    fn raw_from_a_captured_flow_round_trips() {
        let flow = Flow {
            method: "POST".into(),
            req_head: "POST https://a.test/login?x=1 HTTP/1.1\r\nhost: a.test\r\ncontent-type: text/plain\r\n".into(),
            req_body: b"hi".to_vec(),
            ..Flow::default()
        };
        let raw = raw_from_flow(&flow).unwrap();
        assert_eq!(
            raw,
            "POST /login?x=1 HTTP/1.1\nhost: a.test\ncontent-type: text/plain\n\nhi"
        );
        assert_eq!(parse(&raw).unwrap().body, b"hi");
        let tunnel = Flow {
            method: "CONNECT".into(),
            ..Flow::default()
        };
        assert!(raw_from_flow(&tunnel).is_err());
    }

    #[test]
    fn targets_are_validated() {
        let t = |scheme: &str, host: &str, port| Target {
            scheme: scheme.into(),
            host: host.into(),
            port,
        };
        assert!(t("https", "a.test", 443).validate().is_ok());
        assert!(t("ftp", "a.test", 21).validate().is_err());
        assert!(t("http", "a.test/x", 80).validate().is_err());
        assert_eq!(t("https", "a.test", 443).authority(), "a.test");
        assert_eq!(t("http", "::1", 8080).authority(), "[::1]:8080");
    }

    /// Feeds a canned response through `exchange` (over an in-memory pipe).
    async fn respond_with(response: &'static [u8], head_request: bool) -> Received {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let mut sink = [0u8; 1024];
            let _ = server.read(&mut sink).await;
            server.write_all(response).await.unwrap();
        });
        exchange(client, b"GET / HTTP/1.1\r\n\r\n", head_request, 1024)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn reads_every_kind_of_response_body() {
        let r = respond_with(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello", false).await;
        assert_eq!(
            (r.status, r.body.data.as_slice(), r.error),
            (200, &b"hello"[..], None)
        );

        let r = respond_with(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;x=y\r\nabc\r\n2\r\nde\r\n0\r\nTrailer: 1\r\n\r\n", false).await;
        assert_eq!(r.body.data, b"abcde");

        let r = respond_with(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nX-Case: Kept\r\nContent-Length: 2\r\n\r\nok", false).await;
        assert_eq!(r.status, 201);
        assert!(
            r.head.contains("X-Case: Kept"),
            "response header case is kept"
        );

        let r = respond_with(b"HTTP/1.0 200 OK\r\n\r\nuntil close", false).await;
        assert_eq!(r.body.data, b"until close");

        let r = respond_with(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n", true).await;
        assert!(
            r.body.data.is_empty() && r.error.is_none(),
            "HEAD responses have no body"
        );

        let r = respond_with(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort", false).await;
        assert!(r.error.unwrap().contains("5 of 10"));
    }
}
