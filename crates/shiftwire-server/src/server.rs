//! The daemon: wires shiftwire-core to the HTTP adapters and runs until told to stop.

use std::future::IntoFuture;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use shiftwire_core::{
    AgentMode, App, Config, Endpoint, Filter, FlowDetail, RepeaterTab, Scope, Status, Target,
    TimelineEntry, VERSION,
};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::daemon::{self, DaemonState, Paths};
use crate::{guard, mcp, ui, ws};

/// Shared by every request handler.
pub struct Shared {
    pub app: App,
    pub token: String,
    /// Flips to `true` once; everything long-lived (WebSockets) watches it.
    pub stop: watch::Sender<bool>,
    /// MCP session id -> agent name.
    pub mcp_sessions: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// Required from remote clients when the API listens beyond loopback.
    pub api_token: Option<String>,
    /// `config.toml` and the secrets next to it (the data dir).
    pub data_dir: std::path::PathBuf,
    /// Set when the operator changed the listen addresses: start again after stopping.
    pub restart: std::sync::atomic::AtomicBool,
}

pub type Ctx = Arc<Shared>;

pub fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/shutdown", post(shutdown))
        .route("/api/agent-mode", put(set_agent_mode))
        .route("/api/flows", get(flows))
        .route("/api/flows/{id}", get(flow))
        .route("/api/flows/tags", post(tag_flows))
        .route("/api/flows/color", post(color_flows))
        .route("/api/flows/note", post(note_flows))
        .route("/api/flows/{id}/raw", get(flow_raw))
        .route("/api/repeater/tabs", get(repeater_tabs).post(repeater_open))
        .route(
            "/api/repeater/tabs/{id}",
            put(repeater_update).delete(repeater_close),
        )
        .route("/api/repeater/tabs/{id}/send", post(repeater_send))
        .route("/api/ca.crt", get(ca_cert))
        .route("/api/scope", get(get_scope).put(set_scope))
        .route("/api/rules", get(get_rules).put(set_rules))
        .route("/api/network", get(get_network).put(set_network))
        .route("/api/projects", get(projects).post(create_project))
        .route("/api/projects/switch", post(switch_project))
        .route("/api/projects/delete", post(delete_project))
        .route("/api/sitemap", get(sitemap))
        .route("/api/export/har", get(export_har))
        .route("/api/timeline", get(timeline))
        .route("/api/timeline/message", post(message_agents))
        .route("/api/flows/mark", post(mark_flow))
        .route("/mcp", get(mcp::get).post(mcp::post))
        .route("/ws", get(ws::upgrade))
        .fallback(ui::serve)
        .layer(middleware::from_fn_with_state(ctx.clone(), guard::guard))
        .with_state(ctx)
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "version": VERSION }))
}

async fn status(State(c): State<Ctx>) -> Json<Status> {
    Json(c.app.status())
}

async fn shutdown(State(c): State<Ctx>, headers: HeaderMap) -> StatusCode {
    if !guard::bearer_ok(&headers, &c.token) {
        return StatusCode::UNAUTHORIZED;
    }
    c.stop.send_replace(true);
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
struct ModeRequest {
    mode: AgentMode,
}

/// act | advise | off. Only the operator can change it (agents have no tool for it).
async fn set_agent_mode(State(c): State<Ctx>, Json(r): Json<ModeRequest>) -> ApiResult<StatusCode> {
    c.app.set_agent_mode(r.mode).map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct FlowsQuery {
    limit: Option<usize>,
    filter: Option<String>,
    /// Page backwards: only flows older than this id.
    before: Option<String>,
}

type ApiResult<T> = Result<T, (StatusCode, String)>;

/// Bad input (e.g. an unknown color) is the client's fault; anything else is ours.
fn api_error(e: std::io::Error) -> (StatusCode, String) {
    let status = match e.kind() {
        std::io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
        std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, e.to_string())
}

/// The newest flows, oldest first. The UI loads these, then follows live events.
/// A page of matching flows, oldest first; `X-Total-Count` is the number of all matches.
async fn flows(State(c): State<Ctx>, Query(q): Query<FlowsQuery>) -> ApiResult<impl IntoResponse> {
    let limit = q.limit.unwrap_or(1000).clamp(1, 100_000);
    let filter = Filter::parse(q.filter.as_deref().unwrap_or_default())
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let flows = c
        .app
        .query_page(&filter, limit, q.before.as_deref())
        .map_err(api_error)?;
    let total = c.app.count_flows(&filter).map_err(api_error)?;
    Ok(([("x-total-count", total.to_string())], Json(flows)))
}

/// Who changes metadata through the REST API. Agents get their own name via MCP.
const HUMAN: &str = "human";

#[derive(Deserialize)]
struct TagRequest {
    ids: Vec<String>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

async fn tag_flows(State(c): State<Ctx>, Json(r): Json<TagRequest>) -> ApiResult<StatusCode> {
    c.app
        .tag_flows(&r.ids, &r.add, &r.remove, HUMAN)
        .map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ColorRequest {
    ids: Vec<String>,
    color: Option<String>,
}

async fn color_flows(State(c): State<Ctx>, Json(r): Json<ColorRequest>) -> ApiResult<StatusCode> {
    c.app
        .color_flows(&r.ids, r.color.as_deref())
        .map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NoteRequest {
    ids: Vec<String>,
    note: String,
}

async fn note_flows(State(c): State<Ctx>, Json(r): Json<NoteRequest>) -> ApiResult<StatusCode> {
    c.app.note_flows(&r.ids, &r.note).map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// One flow with decoded bodies, for the detail pane.
async fn flow(
    State(c): State<Ctx>,
    Path(id): Path<String>,
) -> Result<Json<FlowDetail>, (StatusCode, String)> {
    match c.app.flow_detail(&id) {
        Ok(Some(detail)) => Ok(Json(detail)),
        Ok(None) => Err((StatusCode::NOT_FOUND, format!("no flow {id}"))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// A captured request as editable text (what "Send to Repeater" copies).
async fn flow_raw(State(c): State<Ctx>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    match c.app.flow_raw(&id).map_err(api_error)? {
        Some(raw) => Ok(Json(json!({ "raw": raw }))),
        None => Err((StatusCode::NOT_FOUND, format!("no flow {id}"))),
    }
}

async fn repeater_tabs(State(c): State<Ctx>) -> ApiResult<Json<Vec<RepeaterTab>>> {
    c.app.repeater_tabs().map(Json).map_err(api_error)
}

#[derive(Deserialize, Default)]
struct OpenTab {
    from_flow: Option<String>,
}

async fn repeater_open(
    State(c): State<Ctx>,
    body: Option<Json<OpenTab>>,
) -> ApiResult<Json<RepeaterTab>> {
    let from = body.map(|Json(b)| b).unwrap_or_default().from_flow;
    c.app
        .repeater_open(from.as_deref())
        .map(Json)
        .map_err(api_error)
}

/// Fields to change on a tab; absent fields stay as they are.
#[derive(Deserialize, Default)]
struct TabEdit {
    name: Option<String>,
    target: Option<Target>,
    raw: Option<String>,
    /// Send only: follow redirects (default on, like the Repeater's checkbox).
    follow: Option<bool>,
}

async fn repeater_update(
    State(c): State<Ctx>,
    Path(id): Path<String>,
    Json(e): Json<TabEdit>,
) -> ApiResult<Json<RepeaterTab>> {
    c.app
        .repeater_update(&id, e.name, e.target, e.raw)
        .map(Json)
        .map_err(api_error)
}

async fn repeater_close(State(c): State<Ctx>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    c.app.repeater_close(&id).map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Saves the editor's current text and target, then sends it.
async fn repeater_send(
    State(c): State<Ctx>,
    Path(id): Path<String>,
    body: Option<Json<TabEdit>>,
) -> ApiResult<Json<serde_json::Value>> {
    let edit = body.map(|Json(e)| e).unwrap_or_default();
    if edit.raw.is_some() || edit.target.is_some() || edit.name.is_some() {
        c.app
            .repeater_update(&id, edit.name, edit.target, edit.raw)
            .map_err(api_error)?;
    }
    let (detail, redirects) = c
        .app
        .repeater_send(&id, HUMAN, edit.follow.unwrap_or(true))
        .await
        .map_err(api_error)?;
    let mut json = serde_json::to_value(detail).map_err(|e| api_error(std::io::Error::other(e)))?;
    json["redirects"] = redirects.into();
    Ok(Json(json))
}

/// The flows matching a filter as a HAR file (for browsers, Burp, ZAP, scripts).
async fn export_har(
    State(c): State<Ctx>,
    Query(q): Query<FlowsQuery>,
) -> ApiResult<impl IntoResponse> {
    let filter = Filter::parse(q.filter.as_deref().unwrap_or_default())
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let limit = q.limit.unwrap_or(5000).clamp(1, 20_000);
    let har = crate::har::export(&c.app, &filter, limit).map_err(api_error)?;
    Ok((
        [(
            axum::http::header::CONTENT_DISPOSITION,
            "attachment; filename=\"shiftwire-export.har\"",
        )],
        Json(har),
    ))
}

#[derive(Deserialize)]
struct SitemapQuery {
    host: Option<String>,
}

async fn sitemap(
    State(c): State<Ctx>,
    Query(q): Query<SitemapQuery>,
) -> ApiResult<Json<Vec<Endpoint>>> {
    c.app
        .sitemap(q.host.as_deref())
        .map(Json)
        .map_err(api_error)
}

#[derive(Deserialize)]
struct TimelineQuery {
    limit: Option<usize>,
}

async fn timeline(
    State(c): State<Ctx>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<Vec<TimelineEntry>>> {
    let limit = q.limit.unwrap_or(2000).clamp(1, 20_000);
    c.app.timeline(limit).map(Json).map_err(api_error)
}

#[derive(Deserialize)]
struct MessageRequest {
    text: String,
}

/// The operator tells the agents something; they receive it with their next tool call.
async fn message_agents(
    State(c): State<Ctx>,
    Json(r): Json<MessageRequest>,
) -> ApiResult<Json<TimelineEntry>> {
    c.app.message_agents(&r.text).map(Json).map_err(api_error)
}

#[derive(Deserialize)]
struct MarkRequest {
    id: String,
    severity: String,
    reason: String,
}

async fn mark_flow(
    State(c): State<Ctx>,
    Json(r): Json<MarkRequest>,
) -> ApiResult<Json<TimelineEntry>> {
    c.app
        .mark_flow(HUMAN, &r.id, &r.severity, &r.reason)
        .map(Json)
        .map_err(api_error)
}

async fn get_scope(State(c): State<Ctx>) -> Json<Scope> {
    Json(c.app.scope())
}

/// Only the operator sets the scope (agents can read it via MCP, not change it).
async fn set_scope(State(c): State<Ctx>, Json(scope): Json<Scope>) -> ApiResult<Json<Scope>> {
    c.app.set_scope(scope).map(Json).map_err(api_error)
}

/// Where the UI/API and the proxy listen, plus what other machines need to connect.
async fn get_network(State(c): State<Ctx>) -> Json<serde_json::Value> {
    let config = c.app.config();
    let secret = |name: &str| {
        std::fs::read_to_string(c.data_dir.join(name))
            .ok()
            .map(|s| s.trim().to_string())
    };
    Json(serde_json::json!({
        "api_addr": config.api_addr,
        "proxy_addr": config.proxy_addr,
        "api_token": c.api_token,
        "proxy_password": if config.proxy_addr.ip().is_loopback() { None } else { secret("proxy.password") },
        // WSL2 sits behind NAT by default: "all interfaces" is not the PC's network.
        "wsl": std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|r| r.to_lowercase().contains("microsoft")),
    }))
}

#[derive(Deserialize)]
struct Network {
    api_addr: String,
    proxy_addr: String,
}

/// Checks the new addresses, saves them and restarts the daemon on them.
async fn set_network(
    State(c): State<Ctx>,
    Json(n): Json<Network>,
) -> ApiResult<Json<serde_json::Value>> {
    let bad = |m: String| (StatusCode::BAD_REQUEST, m);
    let parse = |text: &str, what: &str| -> ApiResult<std::net::SocketAddr> {
        let addr: std::net::SocketAddr = text.trim().parse().map_err(|_| {
            bad(format!(
                "{what}: use address:port, e.g. 127.0.0.1:8080 or 0.0.0.0:8080"
            ))
        })?;
        if addr.port() == 0 {
            return Err(bad(format!("{what}: choose a port from 1 to 65535")));
        }
        Ok(addr)
    };
    let api = parse(&n.api_addr, "UI")?;
    let proxy = parse(&n.proxy_addr, "Proxy")?;
    let overlap =
        api.ip() == proxy.ip() || api.ip().is_unspecified() || proxy.ip().is_unspecified();
    if api.port() == proxy.port() && overlap {
        return Err(bad("the UI and the proxy need different ports".into()));
    }
    let current = c.app.config();
    for (new, old, what) in [
        (api, current.api_addr, "UI"),
        (proxy, current.proxy_addr, "proxy"),
    ] {
        // A new port must be free now (the current one is ours, so it cannot be tested).
        if new.port() != old.port() {
            std::net::TcpListener::bind(new)
                .map_err(|e| bad(format!("cannot listen on {new} for the {what}: {e}")))?;
        }
    }
    let path = c.data_dir.join("config.toml");
    let mut config = Config::load_or_create(&path).map_err(api_error)?;
    (config.api_addr, config.proxy_addr) = (api, proxy);
    config.save(&path).map_err(api_error)?;
    if api != current.api_addr || proxy != current.proxy_addr {
        info!("listen addresses changed (UI {api}, proxy {proxy}): restarting");
        c.restart.store(true, std::sync::atomic::Ordering::Relaxed);
        c.stop.send_replace(true);
    }
    Ok(Json(
        serde_json::json!({ "api_addr": api, "proxy_addr": proxy }),
    ))
}

/// Match & replace / alert rules of the current project (operator only, not over MCP).
async fn get_rules(State(c): State<Ctx>) -> Json<Vec<shiftwire_core::Rule>> {
    Json(c.app.rules())
}

async fn set_rules(
    State(c): State<Ctx>,
    Json(rules): Json<Vec<shiftwire_core::Rule>>,
) -> ApiResult<Json<Vec<shiftwire_core::Rule>>> {
    c.app.set_rules(rules).map(Json).map_err(api_error)
}

/// Projects are folders; only the operator (UI/REST) manages them, never agents over MCP.
async fn projects(State(c): State<Ctx>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "current": c.app.project(),
        "dir": c.app.projects_dir(),
        "projects": c.app.projects(),
    }))
}

#[derive(Deserialize)]
struct NewProject {
    name: String,
    /// Parent folder; default: the projects dir.
    #[serde(default)]
    folder: Option<String>,
    #[serde(default)]
    scope: Scope,
}

async fn create_project(
    State(c): State<Ctx>,
    Json(p): Json<NewProject>,
) -> ApiResult<Json<shiftwire_core::Project>> {
    let folder = p.folder.as_deref().map(str::trim).filter(|f| !f.is_empty());
    c.app
        .create_project(&p.name, folder.map(std::path::Path::new), p.scope)
        .map(Json)
        .map_err(api_error)
}

#[derive(Deserialize)]
struct ProjectPath {
    path: String,
}

async fn switch_project(
    State(c): State<Ctx>,
    Json(p): Json<ProjectPath>,
) -> ApiResult<Json<shiftwire_core::Project>> {
    c.app
        .switch_project(std::path::Path::new(p.path.trim()))
        .map(Json)
        .map_err(api_error)
}

async fn delete_project(State(c): State<Ctx>, Json(p): Json<ProjectPath>) -> ApiResult<StatusCode> {
    c.app
        .delete_project(std::path::Path::new(&p.path))
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(api_error)
}

/// The shiftWire CA certificate, for clients that should have HTTPS intercepted.
async fn ca_cert(State(c): State<Ctx>) -> impl axum::response::IntoResponse {
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/x-x509-ca-cert",
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"shiftwire-ca.crt\"",
            ),
        ],
        c.app.ca_cert_pem().to_string(),
    )
}

/// Entry point of `shiftwire run`.
pub fn run_daemon(paths: &Paths, detached: bool) -> Result<()> {
    let lock = daemon::acquire(paths)?;
    let _log_guard = init_logging(paths, detached);
    let mut config = Config::load_or_create(&paths.config)
        .with_context(|| format!("cannot load {}", paths.config.display()))?;
    config
        .projects_dir
        .get_or_insert_with(|| paths.projects.clone());
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(serve(paths, config, detached));
    drop(runtime); // all sockets closed before a successor binds them
    match result {
        Ok(true) => {
            drop(lock);
            daemon::spawn_detached(paths)
        }
        Ok(false) => Ok(()),
        Err(e) => {
            tracing::error!("{e:#}");
            Err(e)
        }
    }
}

/// Returns whether to start again (the operator changed the listen addresses).
async fn serve(paths: &Paths, config: Config, detached: bool) -> Result<bool> {
    let listener = TcpListener::bind(config.api_addr).await.with_context(|| {
        format!(
            "cannot listen on {} (is something else using it?)",
            config.api_addr
        )
    })?;
    let app = App::open(config.clone(), paths.root.clone()).context("cannot open the project")?;
    let token = daemon::new_token();
    // Listening on all interfaces: local clients (CLI, MCP bridge, browser) use loopback.
    let mut local = config.api_addr;
    if local.ip().is_unspecified() {
        local.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
    }
    let api_url = format!("http://{local}");
    DaemonState {
        pid: std::process::id(),
        version: VERSION.into(),
        api_url: api_url.clone(),
        proxy_addr: config.proxy_addr.to_string(),
        started_at: app.status().started_at,
        token: token.clone(),
    }
    .write(paths)?;

    let (stop, _) = watch::channel(false);
    let ctx = Arc::new(Shared {
        app: app.clone(),
        token,
        stop: stop.clone(),
        mcp_sessions: Default::default(),
        api_token: if config.api_addr.ip().is_loopback() {
            None
        } else {
            Some(daemon::secret(paths, "api.token")?)
        },
        data_dir: paths.root.clone(),
        restart: Default::default(),
    });
    if detached {
        ignore_hangup()?;
    }
    start_proxy(&ctx.app, paths, stop.subscribe()).await;
    info!(
        "shiftWire {VERSION} running: UI/API/MCP {api_url}, data {}",
        paths.root.display()
    );

    let server = axum::serve(listener, router(ctx.clone()))
        .with_graceful_shutdown(stop_requested(stop.clone()))
        .into_future();
    let mut stopped = stop.subscribe();
    let deadline = async move {
        let _ = stopped.wait_for(|s| *s).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    };
    tokio::select! {
        result = server => result?,
        () = deadline => warn!("connections did not close within 5 s; stopping anyway"),
    }
    app.close(); // flush captures still in the write queue
    let _ = std::fs::remove_file(&paths.state);
    info!("shiftWire stopped");
    Ok(ctx.restart.load(std::sync::atomic::Ordering::Relaxed))
}

/// A busy proxy port is not fatal: the UI, API and history stay usable, and
/// `shiftwire status` shows why the proxy is down.
async fn start_proxy(app: &App, paths: &Paths, stop: watch::Receiver<bool>) {
    let addr = app.config().proxy_addr;
    if !addr.ip().is_loopback() {
        // Reachable from other machines: they need a password (local clients do not).
        match daemon::secret(paths, "proxy.password") {
            Ok(password) => app.set_proxy_password(Some(password)),
            Err(e) => warn!("cannot create the proxy password: {e}"),
        }
    }
    match TcpListener::bind(addr).await {
        Ok(listener) => {
            info!("proxy listening on {addr}");
            let app = app.clone();
            tokio::spawn(async move { app.serve_proxy(listener, stop).await });
        }
        Err(e) => {
            let message = format!("cannot listen on {addr}: {e}");
            warn!("proxy disabled: {message}");
            app.set_proxy_error(Some(message));
        }
    }
}

/// Resolves on Ctrl+C, SIGTERM, or `POST /api/shutdown`, and tells everyone.
async fn stop_requested(stop: watch::Sender<bool>) {
    let mut rx = stop.subscribe();
    #[cfg(unix)]
    let term = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => drop(s.recv().await),
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        () = term => {},
        _ = rx.wait_for(|s| *s) => {},
    }
    info!("shutting down");
    stop.send_replace(true);
}

/// A background daemon must not die when the terminal that started it closes.
fn ignore_hangup() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut hup = signal(SignalKind::hangup())?;
        tokio::spawn(async move {
            while hup.recv().await.is_some() {
                info!("ignoring SIGHUP");
            }
        });
    }
    Ok(())
}

fn init_logging(
    paths: &Paths,
    detached: bool,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_env("SHIFTWIRE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    if detached {
        let file = tracing_appender::rolling::daily(&paths.logs, "shiftwire.log");
        let (writer, guard) = tracing_appender::non_blocking(file);
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(writer)
            .with_ansi(false)
            .init();
        Some(guard)
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_ctx() -> Ctx {
        // One directory per test: tests run in parallel.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("shiftwire-server-test-{}-{n}", std::process::id()));
        let app = App::open(Config::default(), dir.clone()).unwrap();
        Arc::new(Shared {
            app,
            token: "t0ken".into(),
            stop: watch::channel(false).0,
            mcp_sessions: Default::default(),
            api_token: None,
            data_dir: dir.clone(),
            restart: Default::default(),
        })
    }

    fn req(method: &str, path: &str, host: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header("host", host)
            .body(Body::empty())
            .unwrap()
    }

    fn json_req(path: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("host", "127.0.0.1:7777")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn projects_are_created_switched_and_deleted() {
        let ctx = test_ctx();
        let first = ctx.app.project();
        let body = serde_json::json!({ "name": "ACME", "scope": { "include": ["acme.test"], "exclude": [] } });
        let res = router(ctx.clone())
            .oneshot(json_req("/api/projects", body))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(ctx.app.status().project, "ACME");
        assert_eq!(ctx.app.scope().include, ["acme.test"]);

        let not_a_project = serde_json::json!({ "path": "/nonexistent/folder" });
        let res = router(ctx.clone())
            .oneshot(json_req("/api/projects/switch", not_a_project))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let loaded = serde_json::json!({ "path": ctx.app.project().path });
        let res = router(ctx.clone())
            .oneshot(json_req("/api/projects/delete", loaded))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "the loaded project stays"
        );

        let back = serde_json::json!({ "path": first.path });
        let res = router(ctx.clone())
            .oneshot(json_req("/api/projects/switch", back))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(ctx.app.project(), first);
    }

    #[tokio::test]
    async fn listen_addresses_are_checked_saved_and_restart_the_daemon() {
        let ctx = test_ctx();
        let put = |body: serde_json::Value| {
            let mut r = json_req("/api/network", body);
            *r.method_mut() = axum::http::Method::PUT;
            r
        };
        for (api, proxy) in [
            ("127.0.0.1:9000", "127.0.0.1:9000"),
            ("nonsense", "127.0.0.1:8080"),
            ("127.0.0.1:0", "127.0.0.1:8080"),
        ] {
            let res = router(ctx.clone())
                .oneshot(put(
                    serde_json::json!({ "api_addr": api, "proxy_addr": proxy }),
                ))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{api} {proxy}");
        }
        assert!(!*ctx.stop.borrow(), "bad input changes nothing");

        let free = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let body =
            serde_json::json!({ "api_addr": "127.0.0.1:7777", "proxy_addr": free.to_string() });
        let res = router(ctx.clone()).oneshot(put(body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let saved = Config::load_or_create(&ctx.data_dir.join("config.toml")).unwrap();
        assert_eq!(saved.proxy_addr, free);
        assert!(*ctx.stop.borrow() && ctx.restart.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn health_is_served_locally() {
        let res = router(test_ctx())
            .oneshot(req("GET", "/api/health", "127.0.0.1:7777"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn foreign_host_and_origin_are_rejected() {
        let res = router(test_ctx())
            .oneshot(req("GET", "/api/status", "evil.example"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let mut r = req("GET", "/api/status", "localhost:7777");
        r.headers_mut()
            .insert("origin", "https://evil.example".parse().unwrap());
        let res = router(test_ctx()).oneshot(r).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn shutdown_needs_the_token() {
        let ctx = test_ctx();
        let res = router(ctx.clone())
            .oneshot(req("POST", "/api/shutdown", "127.0.0.1"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert!(!*ctx.stop.borrow());

        let mut r = req("POST", "/api/shutdown", "127.0.0.1");
        r.headers_mut()
            .insert("authorization", "Bearer t0ken".parse().unwrap());
        let res = router(ctx.clone()).oneshot(r).await.unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        assert!(*ctx.stop.borrow());
    }

    #[tokio::test]
    async fn bad_filters_and_colors_are_400() {
        let res = router(test_ctx())
            .oneshot(req("GET", "/api/flows?filter=hots:x", "127.0.0.1"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let mut r = req("POST", "/api/flows/color", "127.0.0.1");
        *r.body_mut() = Body::from(r#"{"ids":["x"],"color":"pink"}"#);
        r.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        let res = router(test_ctx()).oneshot(r).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn repeater_tabs_open_edit_and_close() {
        let ctx = test_ctx();
        let json_req = |method: &str, path: &str, body: &str| {
            let mut r = req(method, path, "127.0.0.1");
            *r.body_mut() = Body::from(body.to_string());
            r.headers_mut()
                .insert("content-type", "application/json".parse().unwrap());
            r
        };
        let res = router(ctx.clone())
            .oneshot(json_req("POST", "/api/repeater/tabs", "{}"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let id = ctx.app.repeater_tabs().unwrap()[0].id.clone();

        let edit = r#"{"name":"login","target":{"scheme":"https","host":"a.test","port":8443}}"#;
        let res = router(ctx.clone())
            .oneshot(json_req("PUT", &format!("/api/repeater/tabs/{id}"), edit))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let tab = &ctx.app.repeater_tabs().unwrap()[0];
        assert_eq!((tab.name.as_str(), tab.target.port), ("login", 8443));

        let bad = r#"{"target":{"scheme":"ftp","host":"a.test","port":21}}"#;
        let res = router(ctx.clone())
            .oneshot(json_req("PUT", &format!("/api/repeater/tabs/{id}"), bad))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let res = router(ctx.clone())
            .oneshot(req(
                "DELETE",
                &format!("/api/repeater/tabs/{id}"),
                "127.0.0.1",
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert!(ctx.app.repeater_tabs().unwrap().is_empty());
    }

    async fn mcp_call(ctx: &Ctx, session: Option<&str>, body: Value) -> (Option<String>, Value) {
        let mut r = req("POST", "/mcp?agent=Claude%20Code", "127.0.0.1");
        *r.body_mut() = Body::from(body.to_string());
        r.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        if let Some(s) = session {
            r.headers_mut().insert("mcp-session-id", s.parse().unwrap());
        }
        let res = router(ctx.clone()).oneshot(r).await.unwrap();
        let session = res
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().unwrap().to_string());
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            session,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn mcp_intents_bulk_tagging_and_mode_news() {
        let ctx = test_ctx();
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"x"}}});
        let session = mcp_call(&ctx, None, init).await.0.unwrap();
        let call = |name: &str, args: Value| json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":args}});
        let text = |v: &Value| {
            v["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        };

        let (_, r) = mcp_call(
            &ctx,
            Some(&session),
            call("log_intent", json!({"text":"ffuf run"})),
        )
        .await;
        let reply = text(&r);
        assert!(reply.contains("filter intent:"), "{reply}");
        let id = reply
            .split("Intent id ")
            .nth(1)
            .unwrap()
            .split(':')
            .next()
            .unwrap()
            .to_string();

        let (_, r) = mcp_call(
            &ctx,
            Some(&session),
            call(
                "tag_flows",
                json!({"filter": format!("intent:{id}"), "add":["ffuf"]}),
            ),
        )
        .await;
        assert!(
            text(&r).contains("Updated tags on 0 request(s)"),
            "{}",
            text(&r)
        );
        let (_, r) = mcp_call(
            &ctx,
            Some(&session),
            call("tag_flows", json!({"add":["x"]})),
        )
        .await;
        assert_eq!(r["result"]["isError"], true, "ids or a filter is required");

        ctx.app.set_agent_mode(AgentMode::Advise).unwrap();
        let (_, r) = mcp_call(&ctx, Some(&session), call("get_scope", json!({}))).await;
        assert!(
            text(&r).contains("switched AI mode to \"advise\""),
            "{}",
            text(&r)
        );
        let (_, r) = mcp_call(&ctx, Some(&session), call("get_scope", json!({}))).await;
        assert!(!text(&r).contains("switched AI mode"), "told only once");
    }

    #[tokio::test]
    async fn an_mcp_session_lists_tools_and_respects_scope() {
        let ctx = test_ctx();
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"x"}}});
        let (session, reply) = mcp_call(&ctx, None, init).await;
        let session = session.expect("a session id");
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert!(
            reply["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("untrusted")
        );

        let (_, tools) = mcp_call(
            &ctx,
            Some(&session),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await;
        let names: Vec<_> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"send_request") && names.contains(&"list_flows"));
        assert!(
            !names.contains(&"set_scope"),
            "agents cannot change their own scope"
        );

        let call = |name: &str, args: Value| json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":args}});
        let (_, info) = mcp_call(&ctx, Some(&session), call("proxy_info", json!({}))).await;
        let text = info["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("claude-code:x@"),
            "the agent name comes from ?agent=: {text}"
        );

        let (_, refused) = mcp_call(
            &ctx,
            Some(&session),
            call("send_request", json!({"url":"http://127.0.0.1:9/"})),
        )
        .await;
        let text = refused["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("out of scope"), "{text}");

        let (_, marked) = mcp_call(
            &ctx,
            Some(&session),
            call(
                "mark_flow",
                json!({"id":"f1","severity":"high","reason":"admin panel without auth"}),
            ),
        )
        .await;
        assert_eq!(marked["result"]["isError"], false, "{marked}");
        ctx.app
            .message_agents("please stop fuzzing /login")
            .unwrap();
        let (_, next) = mcp_call(&ctx, Some(&session), call("get_scope", json!({}))).await;
        let text = next["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Message from the operator: please stop fuzzing /login"),
            "{text}"
        );

        // Advise mode: no sending, but preparing works and lands in the Repeater.
        ctx.app.set_agent_mode(AgentMode::Advise).unwrap();
        let (_, advised) = mcp_call(
            &ctx,
            Some(&session),
            call("send_request", json!({"url":"http://127.0.0.1:9/"})),
        )
        .await;
        assert!(
            advised["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("advise mode")
        );
        let (_, prepared) = mcp_call(
            &ctx,
            Some(&session),
            call(
                "prepare_request",
                json!({"url":"https://shop.test/api/users/8","name":"read user 8"}),
            ),
        )
        .await;
        assert_eq!(prepared["result"]["isError"], false, "{prepared}");
        assert_eq!(
            ctx.app.repeater_tabs().unwrap()[0].name,
            "claude-code: read user 8"
        );

        let (_, notification) = mcp_call(
            &ctx,
            Some(&session),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
        assert_eq!(notification, Value::Null, "notifications get no body");
    }

    #[tokio::test]
    async fn remote_access_needs_the_token_and_cookies_cannot_be_used_cross_site() {
        let mut shared = Arc::try_unwrap(test_ctx()).ok().unwrap();
        shared.api_token = Some("tok".into());
        let ctx = Arc::new(shared);
        let remote = |method: &str, path: &str| req(method, path, "192.168.1.5:7777");

        let res = router(ctx.clone())
            .oneshot(remote("GET", "/api/status"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let mut r = remote("GET", "/api/status");
        r.headers_mut()
            .insert("authorization", "Bearer tok".parse().unwrap());
        assert_eq!(
            router(ctx.clone()).oneshot(r).await.unwrap().status(),
            StatusCode::OK
        );

        let res = router(ctx.clone())
            .oneshot(remote("GET", "/?token=tok"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert!(
            res.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .contains("HttpOnly; SameSite=Strict")
        );
        let res = router(ctx.clone())
            .oneshot(remote("GET", "/?token=nope"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let mut r = remote("GET", "/api/status");
        r.headers_mut()
            .insert("cookie", "shiftwire_token=tok".parse().unwrap());
        assert_eq!(
            router(ctx.clone()).oneshot(r).await.unwrap().status(),
            StatusCode::OK
        );

        let mut r = remote("POST", "/api/timeline/message");
        r.headers_mut()
            .insert("cookie", "shiftwire_token=tok".parse().unwrap());
        r.headers_mut()
            .insert("origin", "https://evil.example".parse().unwrap());
        assert_eq!(
            router(ctx.clone()).oneshot(r).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn unknown_flow_is_404() {
        let res = router(test_ctx())
            .oneshot(req("GET", "/api/flows/nope", "127.0.0.1"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn ca_certificate_downloads() {
        let res = router(test_ctx())
            .oneshot(req("GET", "/api/ca.crt", "127.0.0.1"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], "application/x-x509-ca-cert");
    }

    #[tokio::test]
    async fn unknown_api_paths_are_404_not_the_ui() {
        let res = router(test_ctx())
            .oneshot(req("GET", "/api/nope", "127.0.0.1"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
