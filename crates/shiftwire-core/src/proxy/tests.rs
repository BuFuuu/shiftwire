//! End-to-end: real sockets, a throwaway upstream, a real store.

use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

use crate::{App, Config, Event, FlowSummary, Scope};

fn scope_of(hosts: &[&str]) -> Scope {
    Scope {
        include: hosts.iter().map(|h| h.to_string()).collect(),
        exclude: vec![],
    }
}

fn app_with(name: &str, config: Config) -> App {
    let dir = std::env::temp_dir().join(format!("shiftwire-proxy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    App::open(config, dir).unwrap()
}

async fn start_proxy(app: &App) -> (SocketAddr, watch::Sender<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, rx) = watch::channel(false);
    let app = app.clone();
    tokio::spawn(async move { app.serve_proxy(listener, rx).await });
    (addr, stop)
}

/// An HTTP server that answers every request with the raw request it received.
async fn echo_http() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    buf.extend_from_slice(&chunk[..n]);
                    if n == 0 || request_complete(&buf) {
                        break;
                    }
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    buf.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&buf).await;
            });
        }
    });
    addr
}

fn request_complete(buf: &[u8]) -> bool {
    let text = String::from_utf8_lossy(buf);
    let Some(end) = text.find("\r\n\r\n") else {
        return false;
    };
    let len = text[..end]
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap_or(0))
        })
        .unwrap_or(0);
    buf.len() >= end + 4 + len
}

/// A TCP server that echoes bytes back (for CONNECT tunnels).
async fn echo_tcp() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

async fn send_raw(proxy: SocketAddr, request: &str) -> String {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

async fn next_flow(rx: &mut broadcast::Receiver<Event>) -> FlowSummary {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            Event::FlowAdded { flow } => return *flow,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn forwards_http_and_records_the_flow() {
    let app = app_with("forward", Config::default());
    app.set_scope(scope_of(&["127.0.0.1"])).unwrap(); // the agent "claude" needs a scope
    let plan = app.log_intent("claude", "Fetch the hello page").unwrap();
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let up = echo_http().await;
    let creds = base64::engine::general_purpose::STANDARD.encode("claude:secret");

    let resp = send_raw(
        proxy,
        &format!(
            "GET http://{up}/hello?x=1 HTTP/1.1\r\nHost: {up}\r\nProxy-Authorization: Basic {creds}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
    assert!(
        resp.contains("GET /hello?x=1 HTTP/1.1"),
        "upstream gets origin-form: {resp}"
    );
    assert!(
        !resp.to_lowercase().contains("proxy-authorization"),
        "credentials must not be forwarded"
    );

    let flow = next_flow(&mut events).await;
    assert_eq!(flow.actor, "claude");
    assert_eq!(flow.seq, 1, "live events carry the # too");
    assert_eq!(
        flow.intent.as_deref(),
        Some(plan.id.as_str()),
        "agent requests carry their intent"
    );
    assert_eq!(
        (flow.method.as_str(), flow.path.as_str()),
        ("GET", "/hello?x=1")
    );
    assert_eq!(flow.status, Some(200));
    assert_eq!(flow.mime.as_deref(), Some("text/plain"));
    assert_eq!(
        flow.error, None,
        "a complete Content-Length body is not an error"
    );

    let full = app.flow(&flow.id).unwrap().unwrap();
    assert!(String::from_utf8_lossy(&full.resp_body).contains("GET /hello?x=1"));
    assert!(
        !full.req_head.to_lowercase().contains("proxy-authorization"),
        "credentials must not be stored"
    );
    assert_eq!(app.status().flows, 1);
}

#[tokio::test]
async fn large_bodies_are_stored_truncated_but_forwarded_whole() {
    let config = Config {
        max_body_bytes: 4,
        ..Config::default()
    };
    let app = app_with("truncate", config);
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let up = echo_http().await;

    let resp = send_raw(
        proxy,
        &format!("POST http://{up}/login HTTP/1.1\r\nHost: {up}\r\nContent-Length: 8\r\nConnection: close\r\n\r\nabcdefgh"),
    )
    .await;
    assert!(
        resp.ends_with("abcdefgh"),
        "upstream got the full body: {resp}"
    );

    let flow = next_flow(&mut events).await;
    let full = app.flow(&flow.id).unwrap().unwrap();
    assert_eq!(full.req_body, b"abcd");
    assert_eq!(full.req_size, 8);
    assert!(full.req_trunc);
    assert_eq!(full.resp_body.len(), 4);
    assert!(full.resp_trunc);
    assert_eq!(flow.actor, "proxy");
    assert_eq!(flow.error, None);
}

#[tokio::test]
async fn unreachable_upstream_is_a_502_and_still_recorded() {
    let app = app_with("unreachable", Config::default());
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap(); // dropped: nothing listens

    let resp = send_raw(
        proxy,
        &format!("GET http://{closed}/ HTTP/1.1\r\nHost: {closed}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 502"), "{resp}");
    let flow = next_flow(&mut events).await;
    assert!(flow.error.is_some());
    assert_eq!(flow.status, None);
}

#[tokio::test]
async fn connect_tunnels_bytes_and_records_on_close() {
    let app = app_with("tunnel", Config::default());
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let echo = echo_tcp().await;

    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = [0u8; 256];
    let n = s.read(&mut head).await.unwrap();
    assert!(String::from_utf8_lossy(&head[..n]).starts_with("HTTP/1.1 200"));

    s.write_all(b"ping").await.unwrap();
    let mut pong = [0u8; 4];
    s.read_exact(&mut pong).await.unwrap();
    assert_eq!(&pong, b"ping");
    drop(s);

    let flow = next_flow(&mut events).await;
    assert_eq!(flow.method, "CONNECT");
    assert_eq!(flow.port, echo.port());
    assert_eq!(flow.size, 4);
}

#[tokio::test]
async fn requests_to_shiftwire_itself_are_refused() {
    let app = app_with("loop", Config::default());
    let (proxy, _stop) = start_proxy(&app).await;
    let resp = send_raw(proxy, "GET http://127.0.0.1:7777/api/status HTTP/1.1\r\nHost: 127.0.0.1:7777\r\nConnection: close\r\n\r\n").await;
    assert!(resp.starts_with("HTTP/1.1 508"), "{resp}");
}

// ---- HTTPS interception ----------------------------------------------------

use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

fn ring() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// An HTTPS server for "localhost" with its own self-signed cert; answers with the request line.
async fn tls_upstream() -> (SocketAddr, CertificateDer<'static>) {
    let issued = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert = issued.cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(issued.signing_key.serialize_der().into());
    let config = ServerConfig::builder_with_provider(ring())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(s).await else {
                    return;
                };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !String::from_utf8_lossy(&buf).contains("\r\n\r\n") {
                    let n = tls.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let first = String::from_utf8_lossy(&buf)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let body = format!("secure: {first}");
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(resp.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (addr, cert)
}

/// CONNECT through the proxy, then speak TLS to "localhost" trusting only `roots`.
async fn https_via_proxy(
    proxy: SocketAddr,
    port: u16,
    roots: RootCertStore,
) -> std::io::Result<String> {
    let mut tcp = TcpStream::connect(proxy).await?;
    let creds = base64::engine::general_purpose::STANDARD.encode("claude:x");
    tcp.write_all(
        format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost:{port}\r\nProxy-Authorization: Basic {creds}\r\n\r\n")
            .as_bytes(),
    )
    .await?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        tcp.read_exact(&mut byte).await?;
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 200"));

    let config = ClientConfig::builder_with_provider(ring())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from("localhost").unwrap();
    let mut tls = TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await?;
    tls.write_all(
        format!("GET /secret?x=1 HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await?;
    let mut out = Vec::new();
    let _ = tls.read_to_end(&mut out).await; // some servers skip close_notify
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn trusting(pem_or_der: &CertificateDer<'static>) -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.add(pem_or_der.clone()).unwrap();
    roots
}

#[tokio::test]
async fn intercepts_https_and_records_the_decrypted_request() {
    let app = app_with("mitm", Config::default());
    app.set_scope(scope_of(&["localhost"])).unwrap();
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let (up, _) = tls_upstream().await;
    let shiftwire_ca = CertificateDer::from_pem_slice(app.ca_cert_pem().as_bytes()).unwrap();

    let resp = https_via_proxy(proxy, up.port(), trusting(&shiftwire_ca))
        .await
        .unwrap();
    assert!(resp.starts_with("HTTP/1.1 200 OK"), "{resp}");
    assert!(resp.ends_with("secure: GET /secret?x=1 HTTP/1.1"), "{resp}");

    let flow = next_flow(&mut events).await;
    assert_eq!(
        (flow.scheme.as_str(), flow.host.as_str()),
        ("https", "localhost")
    );
    assert_eq!(flow.path, "/secret?x=1");
    assert_eq!(flow.seq, 1, "the decrypted CONNECT itself takes no #");
    assert_eq!(
        flow.actor, "claude",
        "actor comes from the CONNECT credentials"
    );
    assert_eq!(flow.status, Some(200));
    assert_eq!(flow.error, None);
}

#[tokio::test]
async fn a_client_that_does_not_trust_shiftwire_is_recorded_with_an_explanation() {
    let app = app_with("untrusted", Config::default());
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let (up, upstream_cert) = tls_upstream().await;

    // The client trusts only the real server, so shiftWire's certificate is rejected.
    assert!(
        https_via_proxy(proxy, up.port(), trusting(&upstream_cert))
            .await
            .is_err()
    );
    let flow = next_flow(&mut events).await;
    assert_eq!(flow.method, "CONNECT");
    assert!(
        flow.error
            .as_deref()
            .unwrap_or_default()
            .contains("does not trust the shiftWire CA"),
        "{flow:?}"
    );
}

#[tokio::test]
async fn passthrough_hosts_are_tunneled_untouched() {
    let config = Config {
        passthrough: vec!["localhost".into()],
        ..Config::default()
    };
    let app = app_with("passthrough", config);
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let (up, upstream_cert) = tls_upstream().await;

    // Succeeds only if the client talks to the real server, not to shiftWire.
    let resp = https_via_proxy(proxy, up.port(), trusting(&upstream_cert))
        .await
        .unwrap();
    assert!(resp.ends_with("secure: GET /secret?x=1 HTTP/1.1"), "{resp}");
    let flow = next_flow(&mut events).await;
    assert_eq!(flow.method, "CONNECT");
    assert_eq!(flow.error, None);
    assert!(flow.size > 0);
}

#[tokio::test]
async fn shiftwire_page_serves_the_ca_certificate() {
    let app = app_with("shiftwirepage", Config::default());
    let (proxy, _stop) = start_proxy(&app).await;
    let page = send_raw(
        proxy,
        "GET http://shiftwire/ HTTP/1.1\r\nHost: shiftwire\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(page.contains(&app.status().ca_fingerprint));
    let crt = send_raw(
        proxy,
        "GET http://shiftwire/ca.crt HTTP/1.1\r\nHost: shiftwire\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(crt.contains(app.ca_cert_pem().trim()));
}

// ---- Repeater ----------------------------------------------------------------

#[tokio::test]
async fn repeater_sends_the_request_as_written_and_records_it() {
    let app = app_with("repeater", Config::default());
    let mut events = app.subscribe();
    let up = echo_http().await;
    let tab = app.repeater_open(None).unwrap();
    let target = crate::Target {
        scheme: "http".into(),
        host: "127.0.0.1".into(),
        port: up.port(),
    };
    // No Host header and a wrong Content-Length: both get fixed. Header order and case stay.
    let raw = "POST /api/login?next=%2F HTTP/1.1\nX-Zeta: 1\nx-alpha: 2\nX-Dup: a\nX-Dup: b\nContent-Length: 99\nConnection: close\n\nuser=admin";
    app.repeater_update(&tab.id, None, Some(target), Some(raw.into()))
        .unwrap();

    let detail = app.repeater_send(&tab.id, "human", false).await.unwrap().0;
    assert_eq!(detail.summary.status, Some(200));
    let echoed = detail.response.unwrap().body.text.unwrap();
    assert!(
        echoed.starts_with("POST /api/login?next=%2F HTTP/1.1\r\n"),
        "{echoed}"
    );
    let zeta = echoed.find("X-Zeta: 1").expect("header case kept");
    let alpha = echoed.find("x-alpha: 2").expect("header case kept");
    assert!(zeta < alpha, "header order kept: {echoed}");
    assert!(
        echoed.contains("X-Dup: a\r\nX-Dup: b\r\n"),
        "duplicates kept: {echoed}"
    );
    assert!(
        echoed.contains("Content-Length: 10\r\n"),
        "length fixed: {echoed}"
    );
    assert!(
        echoed.contains(&format!("Host: 127.0.0.1:{}", up.port())),
        "host filled in: {echoed}"
    );
    assert!(echoed.ends_with("user=admin"));

    // It shows up in History like any other request, and in the tab's history.
    let flow = next_flow(&mut events).await;
    assert_eq!(
        (flow.actor.as_str(), flow.path.as_str()),
        ("human", "/api/login?next=%2F")
    );
    let tab = app
        .repeater_tabs()
        .unwrap()
        .into_iter()
        .find(|t| t.id == tab.id)
        .unwrap();
    assert_eq!(tab.sends, vec![detail.summary.id.clone()]);
    assert_eq!(app.flow(&flow.id).unwrap().unwrap().source, "repeater");
}

#[tokio::test]
async fn repeater_reports_unreachable_targets_on_the_flow() {
    let app = app_with("repeater-down", Config::default());
    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let tab = app.repeater_open(None).unwrap();
    let target = crate::Target {
        scheme: "http".into(),
        host: "127.0.0.1".into(),
        port: closed.port(),
    };
    app.repeater_update(&tab.id, None, Some(target), None)
        .unwrap();
    let detail = app.repeater_send(&tab.id, "human", false).await.unwrap().0;
    assert!(detail.summary.error.unwrap().contains("could not reach"));
    assert!(detail.response.is_none());
}

#[tokio::test]
async fn opening_a_tab_from_a_captured_flow_copies_the_request() {
    let app = app_with("repeater-copy", Config::default());
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let up = echo_http().await;
    send_raw(
        proxy,
        &format!("PUT http://{up}/item/7 HTTP/1.1\r\nHost: {up}\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbody"),
    )
    .await;
    let flow = next_flow(&mut events).await;
    let tab = app.repeater_open(Some(&flow.id)).unwrap();
    assert_eq!(tab.name, "PUT /item/7");
    assert_eq!(tab.target.port, up.port());
    assert!(tab.raw.starts_with("PUT /item/7 HTTP/1.1\n"), "{}", tab.raw);
    assert!(tab.raw.ends_with("\n\nbody"));
}

// ---- Agents: scope and kill switch -----------------------------------------

#[tokio::test]
async fn agents_are_held_to_scope_and_the_kill_switch_but_humans_are_not() {
    let app = app_with("enforce", Config::default());
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let up = echo_http().await;
    let creds = base64::engine::general_purpose::STANDARD.encode("claude:x");
    let as_agent = format!(
        "GET http://{up}/x HTTP/1.1\r\nHost: {up}\r\nProxy-Authorization: Basic {creds}\r\nConnection: close\r\n\r\n"
    );
    let as_human = format!("GET http://{up}/x HTTP/1.1\r\nHost: {up}\r\nConnection: close\r\n\r\n");

    // No scope yet: the agent is refused, and the attempt is recorded for the operator.
    assert!(send_raw(proxy, &as_agent).await.starts_with("HTTP/1.1 403"));
    let refused = next_flow(&mut events).await;
    assert_eq!(refused.actor, "claude");
    assert!(refused.error.as_deref().unwrap().contains("out of scope"));

    // Human (browser) traffic is never restricted.
    assert!(send_raw(proxy, &as_human).await.starts_with("HTTP/1.1 200"));
    next_flow(&mut events).await;

    // In scope: allowed. Paused: refused again.
    app.set_scope(scope_of(&["127.0.0.1"])).unwrap();
    assert!(send_raw(proxy, &as_agent).await.starts_with("HTTP/1.1 200"));
    next_flow(&mut events).await;
    app.set_ai_paused(true);
    assert!(send_raw(proxy, &as_agent).await.starts_with("HTTP/1.1 403"));
    let paused = next_flow(&mut events).await;
    assert!(paused.error.as_deref().unwrap().contains("paused"));
    assert_eq!(app.status().agents, 1);
}

#[tokio::test]
async fn agent_send_and_repeat_respect_scope() {
    let app = app_with("agent-send", Config::default());
    let mut events = app.subscribe();
    let up = echo_http().await;
    let target = crate::Target {
        scheme: "http".into(),
        host: "127.0.0.1".into(),
        port: up.port(),
    };

    let refused = app
        .agent_send("claude", &target, "GET /a HTTP/1.1\n\n")
        .await
        .unwrap();
    assert!(refused.summary.error.unwrap().contains("out of scope"));
    assert!(refused.response.is_none(), "nothing was sent");

    app.set_scope(scope_of(&["127.0.0.1"])).unwrap();
    let ok = app
        .agent_send(
            "claude",
            &target,
            "POST /a HTTP/1.1\nConnection: close\n\nhi",
        )
        .await
        .unwrap();
    assert_eq!(ok.summary.status, Some(200));
    assert_eq!(ok.source, "mcp");

    // Repeat it with changes.
    let mut edits = crate::Edits {
        path: Some("/b?admin=1".into()),
        body: Some("changed".into()),
        ..Default::default()
    };
    edits.set_headers.insert("X-Test".into(), "1".into());
    for _ in 0..2 {
        next_flow(&mut events).await; // the refused and the first sent flow
    }
    let again = app
        .agent_repeat("claude", &ok.summary.id, &edits)
        .await
        .unwrap();
    let echoed = again.response.unwrap().body.text.unwrap();
    assert!(echoed.starts_with("POST /b?admin=1 HTTP/1.1"), "{echoed}");
    assert!(
        echoed.contains("X-Test: 1") && echoed.ends_with("changed"),
        "{echoed}"
    );
}

// ---- Streaming -----------------------------------------------------------------

/// Long-lived streams (SSE, the way AI APIs answer) must flow chunk by chunk:
/// no buffering until the end, and no cut-off while the stream is quiet.
#[tokio::test]
async fn streamed_responses_pass_through_chunk_by_chunk() {
    let app = app_with("stream", Config::default());
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;

    let (go_on, wait) = tokio::sync::oneshot::channel::<()>();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = s.read(&mut buf).await;
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        s.write_all(b"9\r\ndata: 1\n\n\r\n").await.unwrap();
        wait.await.unwrap(); // the second event only after the client saw the first
        s.write_all(b"9\r\ndata: 2\n\n\r\n0\r\n\r\n").await.unwrap();
    });

    let mut client = TcpStream::connect(proxy).await.unwrap();
    client
        .write_all(
            format!("GET http://{up}/events HTTP/1.1\r\nHost: {up}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut got = Vec::new();
    let mut chunk = [0u8; 1024];
    while !String::from_utf8_lossy(&got).contains("data: 1") {
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut chunk))
            .await
            .expect("the first event must arrive before the stream ends (no buffering)")
            .unwrap();
        assert!(n > 0);
        got.extend_from_slice(&chunk[..n]);
    }
    tokio::time::sleep(Duration::from_millis(1500)).await; // a quiet stream is not cut off
    go_on.send(()).unwrap();
    client.read_to_end(&mut got).await.unwrap();
    assert!(String::from_utf8_lossy(&got).contains("data: 2"));

    let flow = next_flow(&mut events).await;
    let full = app.flow(&flow.id).unwrap().unwrap();
    assert_eq!(full.resp_body, b"data: 1\n\ndata: 2\n\n");
    assert_eq!(flow.error, None);
}

#[tokio::test]
async fn rules_replace_and_alert_before_forwarding() {
    use crate::{Rule, RuleMode, RulePart};
    let app = app_with("rules", Config::default());
    let rule = |name: &str, mode, part, pattern: &str, replace: &str, when_missing| Rule {
        id: String::new(),
        name: name.into(),
        enabled: true,
        mode,
        part,
        pattern: pattern.into(),
        replace: replace.into(),
        when_missing,
        block: false,
    };
    app.set_rules(vec![
        rule(
            "UA",
            RuleMode::Replace,
            RulePart::Headers,
            "^User-Agent: .*$",
            "User-Agent: Mine/1.0",
            false,
        ),
        rule(
            "admin flag",
            RuleMode::Replace,
            RulePart::Body,
            "admin=0",
            "admin=1",
            false,
        ),
        rule(
            "token missing",
            RuleMode::Alert,
            RulePart::Headers,
            "^X-Token: ",
            "",
            true,
        ),
    ])
    .unwrap();
    assert!(
        app.set_rules(vec![rule(
            "bad",
            RuleMode::Alert,
            RulePart::Body,
            "(",
            "",
            false
        )])
        .is_err()
    );
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let up = echo_http().await;

    let resp = send_raw(
        proxy,
        &format!(
            "POST http://{up}/save HTTP/1.1\r\nHost: {up}\r\nUser-Agent: curl/8\r\nContent-Length: 12\r\nConnection: close\r\n\r\nid=7&admin=0"
        ),
    )
    .await;
    assert!(
        resp.to_lowercase().contains("user-agent: mine/1.0"),
        "{resp}"
    );
    assert!(
        resp.ends_with("\r\n\r\nid=7&admin=1"),
        "body rewritten: {resp}"
    );
    assert!(
        resp.to_lowercase().contains("content-length: 12\r\n"),
        "{resp}"
    );

    let mut alert = None;
    let flow = loop {
        match tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            Event::RuleAlert { rule, .. } => alert = Some(rule),
            Event::FlowAdded { flow } => break *flow,
            _ => {}
        }
    };
    assert_eq!(alert.as_deref(), Some("token missing"));
    assert_eq!(
        flow.tags,
        ["replaced:ua", "alert:token-missing", "replaced:admin-flag"]
    );
    let stored = app.flow(&flow.id).unwrap().unwrap();
    assert!(
        stored.req_head.contains("Mine/1.0"),
        "History shows what was sent"
    );
    assert_eq!(stored.tags.len(), 3, "rule tags are stored");
}

#[tokio::test]
async fn blocking_alert_rules_stop_proxied_and_agent_requests() {
    use crate::{Rule, RuleMode, RulePart, Scope, Target};
    let app = app_with("block", Config::default());
    app.set_rules(vec![Rule {
        id: String::new(),
        name: "no passwords".into(),
        enabled: true,
        mode: RuleMode::Alert,
        part: RulePart::Body,
        pattern: "password=".into(),
        replace: String::new(),
        when_missing: false,
        block: true,
    }])
    .unwrap();
    let mut events = app.subscribe();
    let (proxy, _stop) = start_proxy(&app).await;
    let up = echo_http().await;

    let resp = send_raw(
        proxy,
        &format!(
            "POST http://{up}/login HTTP/1.1\r\nHost: {up}\r\nContent-Length: 10\r\nConnection: close\r\n\r\npassword=x"
        ),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    assert!(
        resp.contains("blocked by the alert rule \"no passwords\""),
        "{resp}"
    );
    let (mut alerted, mut flow) = (false, None);
    while flow.is_none() {
        match tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            Event::RuleAlert { blocked, .. } => alerted = blocked,
            Event::FlowAdded { flow: f } => flow = Some(*f),
            _ => {}
        }
    }
    let flow = flow.unwrap();
    assert!(alerted, "the alert says it blocked");
    assert!(flow.error.as_deref().unwrap().contains("blocked"));
    assert_eq!(flow.status, None, "never reached the target");

    // An agent's own send_request obeys the same rule.
    app.set_scope(Scope {
        include: vec!["127.0.0.1".into()],
        exclude: vec![],
    })
    .unwrap();
    let target = Target {
        scheme: "http".into(),
        host: up.ip().to_string(),
        port: up.port(),
    };
    let sent = app
        .agent_send("claude", &target, "POST /login HTTP/1.1\n\npassword=y")
        .await
        .unwrap();
    assert!(
        sent.summary
            .error
            .as_deref()
            .unwrap()
            .contains("no passwords")
    );
    assert!(sent.response.is_none(), "not sent");
    assert_eq!(sent.summary.tags, ["alert:no-passwords"]);
}

#[tokio::test]
async fn the_browser_listener_marks_its_requests_and_is_not_counted_as_proxy_clients() {
    let app = app_with("browser-listener", Config::default());
    let mut events = app.subscribe();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (_stop, rx) = watch::channel(false);
    tokio::spawn(super::serve_browser(app.clone(), listener, rx));
    let up = echo_http().await;
    let resp = send_raw(
        addr,
        &format!("GET http://{up}/page HTTP/1.1\r\nHost: {up}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let flow = next_flow(&mut events).await;
    assert_eq!(flow.via, "browser");
    assert_eq!(app.status().proxy_connections, 0);
    assert_eq!(app.status().proxy_last_request, 0, "not the proxy port");
    assert_eq!(flow.actor, "proxy", "nobody drives it: the operator's");

    // During an agent's browser action (and a moment after), requests are the agent's.
    app.begin_browser_action("claude");
    let get = format!("GET http://{up}/x HTTP/1.1\r\nHost: {up}\r\nConnection: close\r\n\r\n");
    send_raw(addr, &get).await;
    assert_eq!(next_flow(&mut events).await.actor, "claude");
    app.end_browser_action("claude");
    send_raw(addr, &get).await;
    assert_eq!(
        next_flow(&mut events).await.actor,
        "claude",
        "just after the call"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    send_raw(addr, &get).await;
    assert_eq!(next_flow(&mut events).await.actor, "proxy");
}

#[tokio::test]
async fn the_ai_browser_gets_a_welcome_page_instead_of_ca_instructions() {
    let app = app_with("browser-welcome", Config::default());
    let (proxy, _stop) = start_proxy(&app).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let browser = listener.local_addr().unwrap();
    let (_stop2, rx) = watch::channel(false);
    tokio::spawn(super::serve_browser(app.clone(), listener, rx));
    let get = "GET http://shiftwire/ HTTP/1.1\r\nHost: shiftwire\r\nConnection: close\r\n\r\n";
    let welcome = send_raw(browser, get).await;
    assert!(
        welcome.contains("shiftWire AI browser") && !welcome.contains("Download the shiftWire CA")
    );
    let ca_page = send_raw(proxy, get).await;
    assert!(
        ca_page.contains("Download the shiftWire CA"),
        "other clients still get the CA page"
    );
}

#[tokio::test]
async fn masking_keeps_agents_off_the_proxy() {
    use crate::{MaskLevel, MaskSite, Masking};
    let app = app_with("masking-proxy", Config::default());
    app.set_scope(Scope {
        include: vec!["127.0.0.1".into()],
        exclude: vec![],
    })
    .unwrap();
    let (proxy, _stop) = start_proxy(&app).await;
    let up = echo_http().await;
    let creds = base64::engine::general_purpose::STANDARD.encode("claude:x");
    let request = |auth: &str| {
        format!("GET http://{up}/ HTTP/1.1\r\nHost: {up}\r\n{auth}Connection: close\r\n\r\n")
    };
    let agent = format!("Proxy-Authorization: Basic {creds}\r\n");
    app.set_masking(Masking {
        level: MaskLevel::Addresses,
        sites: vec![MaskSite {
            real: "example.test".into(),
            alias: String::new(),
        }],
        ..Masking::default()
    })
    .unwrap();
    let refused = send_raw(proxy, &request(&agent)).await;
    assert!(
        refused.starts_with("HTTP/1.1 403") && refused.contains("masking is on"),
        "{refused}"
    );
    let human = send_raw(proxy, &request("")).await;
    assert!(
        human.starts_with("HTTP/1.1 200"),
        "the operator's own traffic is fine: {human}"
    );
}

#[tokio::test]
async fn repeater_follows_redirects_when_asked() {
    // /start -> 302 /mid, /mid -> 303 http://same/final, /final -> 200 "landed: <method> <path>"
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let line = req.lines().next().unwrap_or_default().to_string();
                let resp = if line.contains(" /start ") {
                    "HTTP/1.1 302 Found
Location: /mid
Content-Length: 0

"
                    .to_string()
                } else if line.contains(" /mid ") {
                    format!(
                        "HTTP/1.1 303 See Other
Location: http://{up}/final
Content-Length: 0

"
                    )
                } else {
                    let body = format!("landed: {line}");
                    format!(
                        "HTTP/1.1 200 OK
Content-Length: {}

{body}",
                        body.len()
                    )
                };
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    let app = app_with("redirects", Config::default());
    let tab = app.repeater_open(None).unwrap();
    let target = crate::Target {
        scheme: "http".into(),
        host: "127.0.0.1".into(),
        port: up.port(),
    };
    let raw = format!(
        "POST /start HTTP/1.1
Host: {up}
Content-Type: text/plain

hello"
    );
    app.repeater_update(&tab.id, None, Some(target), Some(raw))
        .unwrap();

    let (detail, redirects) = app.repeater_send(&tab.id, "human", true).await.unwrap();
    assert_eq!(redirects, 2);
    assert_eq!(detail.summary.status, Some(200));
    let body = detail.response.unwrap().body.text.unwrap();
    assert_eq!(
        body, "landed: GET /final HTTP/1.1",
        "302 after POST and 303 become GET"
    );

    let (detail, redirects) = app.repeater_send(&tab.id, "human", false).await.unwrap();
    assert_eq!(
        (redirects, detail.summary.status),
        (0, Some(302)),
        "unticked: the redirect itself"
    );
}
