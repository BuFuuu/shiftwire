//! The intercepting proxy.
//!
//! Plain HTTP and decrypted HTTPS go through the same [`Proxy::handle`]: after a
//! CONNECT is intercepted, each request on the TLS stream is rewritten to an
//! `https://` absolute URI and handled exactly like a plain proxied request.
//!
//! Rule: capture must never break or slow forwarding. Bodies stream through a
//! [`capture::Tee`]; the finished flow is handed to the store without waiting.

pub(crate) mod ca;
pub(crate) mod capture;
pub(crate) mod tls;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Either, Full};
use hyper::body::{Body, Incoming};
use hyper::header::{self, HeaderMap, HeaderName};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::LazyConfigAcceptor;

use crate::model::Flow;
use crate::rules::{Outcome, Rules};
use crate::{App, now_ms};
use capture::{BodyCapture, Tee};
use tls::{Hello, Prefixed};

type ProxyBody = UnsyncBoxBody<Bytes, hyper::Error>;
/// What goes upstream: the client's body as it streams in, or one rewritten by a body rule.
type ReqBody = Either<Incoming, Full<Bytes>>;

/// A client that gives up on the handshake this long after CONNECT is dropped.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct Proxy {
    app: App,
    client: Client<HttpsConnector<HttpConnector>, Tee<ReqBody>>,
    /// Ports of our own listeners, to refuse requests that would loop back to us.
    own_ports: [u16; 3],
    /// Stored on its flows: `proxy`, or `browser` for the AI browser's listener.
    source: &'static str,
}

/// Where requests on an intercepted TLS stream are going, and who opened it.
struct Target {
    scheme: &'static str,
    host: String,
    port: u16,
    actor: String,
}

pub(crate) async fn serve(app: App, listener: TcpListener, stop: watch::Receiver<bool>) {
    accept_loop(Proxy::new(app), listener, stop).await;
}

/// The AI browser's own listener: the same proxy, its requests marked `browser`.
pub(crate) async fn serve_browser(app: App, listener: TcpListener, stop: watch::Receiver<bool>) {
    let proxy = Proxy {
        source: "browser",
        ..Proxy::new(app)
    };
    accept_loop(proxy, listener, stop).await;
}

async fn accept_loop(proxy: Proxy, listener: TcpListener, mut stop: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => proxy.spawn_connection(stream, peer),
                Err(e) => {
                    tracing::warn!("proxy accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
            () = stopped(&mut stop) => break,
        }
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|s| *s).await;
}

impl Proxy {
    fn new(app: App) -> Self {
        let config = app.config();
        let tls = tls::upstream_config(config.verify_upstream_tls, app.crypto());
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_or_http()
            .enable_http1()
            .build();
        Self {
            own_ports: [
                config.proxy_addr.port(),
                config.api_addr.port(),
                config.mcp_addr.port(),
            ],
            source: "proxy",
            client: Client::builder(TokioExecutor::new())
                .http1_preserve_header_case(true)
                .build(connector),
            app,
        }
    }

    fn spawn_connection(&self, stream: TcpStream, peer: SocketAddr) {
        let proxy = self.clone();
        // The status bar counts the proxy port's clients, not the AI browser's.
        let open = (self.source == "proxy").then(|| self.app.proxy_connection());
        tokio::spawn(async move {
            let _open = open; // counted in status while the client stays connected
            // Clients on this machine need no password; others do once one is set.
            let remote = !peer.ip().is_loopback();
            if let Err(e) = proxy.serve_http(stream, None, remote).await {
                tracing::debug!("proxy connection from {peer}: {e}");
            }
        });
    }

    /// Serves HTTP/1.1 on `io`: the client's plain connection, or a decrypted TLS stream.
    async fn serve_http<I>(
        &self,
        io: I,
        target: Option<Arc<Target>>,
        remote: bool,
    ) -> hyper::Result<()>
    where
        I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let proxy = self.clone();
        let service = service_fn(move |req| {
            let (proxy, target) = (proxy.clone(), target.clone());
            async move { Ok::<_, Infallible>(proxy.handle(req, target.as_deref(), remote).await) }
        });
        http1::Builder::new()
            .preserve_header_case(true)
            .serve_connection(TokioIo::new(io), service)
            .with_upgrades()
            .await
    }

    /// Body rules need the whole body: only small, uncompressed bodies of known
    /// length are read first. Everything else streams through untouched.
    async fn apply_body_rules(
        &self,
        req: Request<Incoming>,
        rules: &Rules,
        outcome: &mut Outcome,
    ) -> Result<Request<ReqBody>, Box<Response<ProxyBody>>> {
        let headers = req.headers();
        let length = headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()?.parse::<usize>().ok());
        let readable = rules.wants_body()
            && length.is_some_and(|n| n > 0 && n <= self.app.config().max_body_bytes)
            && !headers.contains_key(header::TRANSFER_ENCODING)
            && !headers.contains_key(header::CONTENT_ENCODING);
        if !readable {
            return Ok(req.map(Either::Left));
        }
        let (mut parts, body) = req.into_parts();
        let mut bytes = match body.collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(e) => {
                let message = format!("shiftWire could not read the request body: {e}");
                return Err(Box::new(text(StatusCode::BAD_REQUEST, &message)));
            }
        };
        if rules.apply_body(&mut bytes, outcome) {
            parts.headers.insert(
                header::CONTENT_LENGTH,
                header::HeaderValue::from(bytes.len()),
            );
        }
        Ok(Request::from_parts(
            parts,
            Either::Right(Full::new(Bytes::from(bytes))),
        ))
    }

    async fn handle(
        &self,
        mut req: Request<Incoming>,
        target: Option<&Target>,
        remote: bool,
    ) -> Response<ProxyBody> {
        let (mut actor, password) = take_credentials(req.headers_mut());
        if remote && !self.app.proxy_password_ok(password.as_deref()) {
            let mut resp = text(
                StatusCode::PROXY_AUTHENTICATION_REQUIRED,
                "shiftWire needs the proxy password (see `shiftwire status`): http://NAME:PASSWORD@host:port",
            );
            resp.headers_mut().insert(
                header::PROXY_AUTHENTICATE,
                header::HeaderValue::from_static("Basic realm=\"shiftWire\""),
            );
            return resp;
        }
        if let Some(t) = target {
            // Inside a tunnel, the credentials came with the CONNECT.
            if actor == "proxy" {
                actor.clone_from(&t.actor);
            }
            match absolute_uri(t, req.uri()) {
                Some(uri) => *req.uri_mut() = uri,
                None => return text(StatusCode::BAD_REQUEST, "Invalid request target."),
            }
        } else if req.method() == Method::CONNECT {
            return self.connect(req, actor);
        }
        // The AI browser: the agent whose browser action is running, else the operator.
        if self.source == "browser" {
            actor = self.app.browser_actor().unwrap_or_else(|| "proxy".into());
        }

        let uri = req.uri().clone();
        let Some(authority) = uri.authority() else {
            return shiftwire_page(&self.app, "/", false); // origin-form: someone opened the proxy port directly
        };
        if authority.host().eq_ignore_ascii_case("shiftwire") {
            return shiftwire_page(&self.app, uri.path(), self.source == "browser");
        }
        let scheme = match uri.scheme_str() {
            Some(s @ ("http" | "https")) => s.to_string(),
            _ => return text(StatusCode::BAD_REQUEST, "Unsupported URL scheme."),
        };
        let host = authority.host().trim_matches(['[', ']']).to_string();
        let port = authority
            .port_u16()
            .unwrap_or(if scheme == "https" { 443 } else { 80 });
        if self.is_own_listener(&host, port) {
            return text(
                StatusCode::LOOP_DETECTED,
                "That is shiftWire itself; refusing to loop.",
            );
        }

        // Match & replace / alert rules see (and may change) the request before it
        // is forwarded; History then shows the request as it was sent.
        let rules = self.app.compiled_rules();
        let mut outcome = Outcome::default();
        rules.apply_headers(req.headers_mut(), &mut outcome);
        let mut req = match self.apply_body_rules(req, &rules, &mut outcome).await {
            Ok(req) => req,
            Err(resp) => return *resp,
        };

        let mut flow = Flow {
            id: crate::new_id(),
            seq: self.app.next_seq(),
            ts_start: now_ms(),
            actor,
            source: self.source.into(),
            scheme,
            host,
            port,
            method: req.method().to_string(),
            path: uri.path().to_string(),
            query: uri.query().map(str::to_string),
            http_version: format!("{:?}", req.version()),
            req_head: head(
                &format!("{} {} {:?}", req.method(), uri, req.version()),
                req.headers(),
            ),
            tags: std::mem::take(&mut outcome.tags),
            ..Flow::default()
        };
        for rule in &outcome.alerts {
            let blocked = outcome.blocked.as_ref() == Some(rule);
            self.app.rule_alert(rule, &flow, blocked);
        }

        if flow.actor != "proxy" {
            self.app.touch_agent(&flow.actor);
            flow.intent = self.app.current_intent(&flow.actor); // filter: intent:<id>
        }
        // Not for the AI browser: a page's CDN and SSO requests would break. Agents'
        // browser tools are checked instead (mode, and the scope of browser_navigate).
        let mut refused = (self.source != "browser")
            .then(|| self.app.agent_allowed(&flow.actor, &flow.host).err())
            .flatten();
        // Masking: an agent's own tools would see real names, so they cannot use the proxy.
        if refused.is_none()
            && self.source == "proxy"
            && flow.actor != "proxy"
            && self.app.masker().active()
        {
            refused = Some(
                "masking is on: agents use shiftWire's MCP (send_request, repeat_flow, browser_*), not the proxy".into(),
            );
        }
        if let Some(reason) = refused.or_else(|| outcome.block_reason()) {
            flow.error = Some(reason.clone());
            flow.ts_end = now_ms();
            self.app.record(flow);
            return text(StatusCode::FORBIDDEN, &format!("shiftWire: {reason}"));
        }

        strip_hop_by_hop(req.headers_mut());
        let limit = self.app.config().max_body_bytes;
        let req_capture = Arc::new(Mutex::new(BodyCapture::new(limit)));
        let (parts, body) = req.into_parts();
        let upstream_req = Request::from_parts(parts, Tee::new(body, req_capture.clone(), None));

        let resp = match self.client.request(upstream_req).await {
            Ok(resp) => resp,
            Err(e) => {
                let message = format!(
                    "shiftWire could not reach {}:{}: {}",
                    flow.host,
                    flow.port,
                    error_chain(&e)
                );
                fill_body(&mut flow, &req_capture, true);
                flow.error = Some(message.clone());
                flow.ts_end = now_ms();
                self.app.record(flow);
                return text(StatusCode::BAD_GATEWAY, &message);
            }
        };

        let (mut parts, body) = resp.into_parts();
        flow.status = Some(parts.status.as_u16());
        flow.mime = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or(v).trim().to_string());
        flow.resp_head = head(
            &format!("{:?} {}", parts.version, parts.status),
            &parts.headers,
        );
        strip_hop_by_hop(&mut parts.headers);

        let resp_capture = Arc::new(Mutex::new(BodyCapture::new(limit)));
        let app = self.app.clone();
        let done_capture = resp_capture.clone();
        let on_end = move |error: Option<String>| {
            fill_body(&mut flow, &req_capture, true);
            fill_body(&mut flow, &done_capture, false);
            flow.error = error;
            flow.ts_end = now_ms();
            app.record(flow);
        };
        let body = if body.is_end_stream() {
            on_end(None); // empty body (e.g. HEAD, 204): the server may never poll it
            Tee::new(body, resp_capture, None)
        } else {
            Tee::new(body, resp_capture, Some(Box::new(on_end)))
        };
        Response::from_parts(parts, body.boxed_unsync())
    }

    /// CONNECT: accept at once, then decide what to do from the client's first bytes.
    fn connect(&self, req: Request<Incoming>, actor: String) -> Response<ProxyBody> {
        let authority = req
            .uri()
            .authority()
            .map(|a| a.to_string())
            .unwrap_or_default();
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (
                h.trim_matches(['[', ']']).to_string(),
                p.parse().unwrap_or(443),
            ),
            None => (authority.clone(), 443),
        };
        if self.is_own_listener(&host, port) {
            return text(
                StatusCode::LOOP_DETECTED,
                "That is shiftWire itself; refusing to loop.",
            );
        }
        // No `#` yet: decrypted CONNECTs are not stored themselves, only what runs inside.
        let skeleton = Flow {
            id: crate::new_id(),
            ts_start: now_ms(),
            actor,
            // The AI browser's tunnels stay `browser`, so History and the status bar keep them apart.
            source: if self.source == "browser" {
                "browser"
            } else {
                "tunnel"
            }
            .into(),
            scheme: "https".into(),
            host,
            port,
            method: "CONNECT".into(),
            http_version: format!("{:?}", req.version()),
            req_head: head(
                &format!("CONNECT {authority} {:?}", req.version()),
                req.headers(),
            ),
            ..Flow::default()
        };
        let proxy = self.clone();
        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => proxy.after_connect(TokioIo::new(upgraded), skeleton).await,
                Err(e) => tracing::debug!("CONNECT upgrade failed: {e}"),
            }
        });
        Response::new(empty())
    }

    async fn after_connect(&self, mut client: TokioIo<Upgraded>, mut flow: Flow) {
        let hello =
            match tokio::time::timeout(HELLO_TIMEOUT, tls::read_client_hello(&mut client)).await {
                Ok(Ok(hello)) => hello,
                _ => return, // the client went away without sending anything useful
            };
        match hello {
            Hello::Other(bytes) => self.raw_tunnel(client, bytes, flow).await,
            Hello::Tls { sni, bytes } => {
                // A CONNECT to an IP usually carries the real name in SNI.
                if let Some(name) = sni {
                    flow.host = name;
                }
                if self.app.config().is_passthrough(&flow.host) {
                    self.raw_tunnel(client, bytes, flow).await;
                } else {
                    self.intercept(Prefixed::new(bytes, client), flow).await;
                }
            }
        }
    }

    /// Decrypt with a certificate from the shiftWire CA and serve the requests inside.
    async fn intercept(&self, io: Prefixed<TokioIo<Upgraded>>, mut flow: Flow) {
        let handshake = async {
            let config = self.app.ca().server_config(&flow.host)?;
            LazyConfigAcceptor::new(rustls::server::Acceptor::default(), io)
                .await?
                .into_stream(config)
                .await
        };
        match handshake.await {
            Ok(stream) => {
                let target = Arc::new(Target {
                    scheme: "https",
                    host: flow.host,
                    port: flow.port,
                    actor: flow.actor,
                });
                if let Err(e) = self.serve_http(stream, Some(target), false).await {
                    tracing::debug!("intercepted connection: {e}");
                }
            }
            Err(e) => {
                flow.error = Some(format!(
                    "TLS handshake failed ({e}). The client probably does not trust the shiftWire CA \
                     (install it from http://shiftwire/), or the app pins its certificates."
                ));
                flow.ts_end = now_ms();
                self.app.record(flow);
            }
        }
    }

    /// Splice bytes both ways without decrypting; recorded when it closes.
    async fn raw_tunnel(
        &self,
        mut client: TokioIo<Upgraded>,
        first_bytes: Vec<u8>,
        mut flow: Flow,
    ) {
        flow.seq = self.app.next_seq(); // a tunnel is stored: number it when it opens
        let result = async {
            let mut server = TcpStream::connect((flow.host.as_str(), flow.port)).await?;
            server.write_all(&first_bytes).await?;
            tokio::io::copy_bidirectional(&mut client, &mut server).await
        };
        match result.await {
            Ok((sent, received)) => {
                flow.status = Some(200);
                flow.req_size = sent + first_bytes.len() as u64;
                flow.resp_size = received;
            }
            Err(e) => {
                flow.error = Some(format!("tunnel to {}:{} failed: {e}", flow.host, flow.port))
            }
        }
        flow.ts_end = now_ms();
        self.app.record(flow);
    }

    fn is_own_listener(&self, host: &str, port: u16) -> bool {
        let local = matches!(host, "127.0.0.1" | "localhost" | "::1" | "0.0.0.0");
        local && self.own_ports.contains(&port)
    }
}

/// `/path?q` on an intercepted stream → `https://host[:port]/path?q`.
fn absolute_uri(target: &Target, uri: &Uri) -> Option<Uri> {
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    let host = if target.host.contains(':') {
        format!("[{}]", target.host)
    } else {
        target.host.clone()
    };
    let default = if target.scheme == "https" { 443 } else { 80 };
    let authority = if target.port == default {
        host
    } else {
        format!("{host}:{}", target.port)
    };
    format!("{}://{authority}{path}", target.scheme)
        .parse()
        .ok()
}

/// `Proxy-Authorization: Basic base64(agent:password)`: the user names the actor,
/// the password is checked when clients connect from other machines. The header
/// is removed so it is neither forwarded nor stored.
fn take_credentials(headers: &mut HeaderMap) -> (String, Option<String>) {
    let creds = headers
        .remove(header::PROXY_AUTHORIZATION)
        .and_then(|value| {
            let encoded = value
                .to_str()
                .ok()?
                .strip_prefix("Basic ")?
                .trim()
                .to_string();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()?;
            let text = String::from_utf8(decoded).ok()?;
            let (user, password) = text.split_once(':').unwrap_or((&text, ""));
            Some((user.trim().to_string(), password.to_string()))
        });
    match creds {
        Some((user, password)) if !user.is_empty() => (user, Some(password)),
        Some((_, password)) => ("proxy".into(), Some(password)),
        None => ("proxy".into(), None),
    }
}

/// Connection-level headers must not be forwarded (RFC 9110 §7.6.1).
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in [
        "connection",
        "proxy-connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

fn fill_body(flow: &mut Flow, capture: &capture::Shared, request: bool) {
    let mut c = capture.lock().expect("capture lock");
    let (size, trunc) = (c.total, c.truncated());
    let data = std::mem::take(&mut c.data);
    if request {
        (flow.req_body, flow.req_size, flow.req_trunc) = (data, size, trunc);
    } else {
        (flow.resp_body, flow.resp_size, flow.resp_trunc) = (data, size, trunc);
    }
}

/// hyper's client errors hide the useful part ("certificate", "refused") in the source chain.
pub(crate) fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

/// First line plus headers, as they appeared on the wire.
pub(crate) fn head(first_line: &str, headers: &HeaderMap) -> String {
    let mut out = format!("{first_line}\r\n");
    for (name, value) in headers {
        out.push_str(name.as_str());
        out.push_str(": ");
        out.push_str(&String::from_utf8_lossy(value.as_bytes()));
        out.push_str("\r\n");
    }
    out
}

fn empty() -> ProxyBody {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed_unsync()
}

fn respond(status: StatusCode, content_type: &str, body: impl Into<Bytes>) -> Response<ProxyBody> {
    let body = Full::new(body.into()).map_err(|never| match never {});
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(body.boxed_unsync())
        .expect("valid response")
}

fn text(status: StatusCode, message: &str) -> Response<ProxyBody> {
    respond(status, "text/plain; charset=utf-8", format!("{message}\n"))
}

/// `http://shiftwire/` (through the proxy): CA download and setup instructions.
/// `http://shiftwire/`: how to trust the CA, or (in the AI browser, which trusts it
/// already) a short welcome page.
fn shiftwire_page(app: &App, path: &str, ai_browser: bool) -> Response<ProxyBody> {
    let ca = app.ca();
    if ai_browser && path == "/" {
        let html = include_str!("browser_page.html");
        return respond(StatusCode::OK, "text/html; charset=utf-8", html.to_string());
    }
    if path == "/ca.crt" {
        let mut resp = respond(
            StatusCode::OK,
            "application/x-x509-ca-cert",
            ca.cert_pem().to_string(),
        );
        resp.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            header::HeaderValue::from_static("attachment; filename=\"shiftwire-ca.crt\""),
        );
        return resp;
    }
    let html = include_str!("shiftwire_page.html").replace("{{FINGERPRINT}}", ca.fingerprint());
    respond(StatusCode::OK, "text/html; charset=utf-8", html)
}

#[cfg(test)]
mod tests;
