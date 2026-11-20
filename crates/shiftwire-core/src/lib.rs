//! shiftWire core: every piece of logic lives here, behind the [`App`] facade.
//!
//! Rule: this crate knows nothing about HTTP APIs, MCP or the UI. Adapters
//! (REST, WebSocket, MCP, CLI) call `App` and listen on its event bus, so the
//! human and the AI always see the same state.

mod browser;
pub mod config;
mod decode;
pub mod events;
pub mod filter;
pub mod masking;
pub mod model;
pub mod project;
mod proxy;
pub mod rules;
pub mod scope;
pub mod sender;
pub mod sitemap;
mod store;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{fs, io};

use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};

pub use config::Config;
pub use events::Event;
pub use filter::{Filter, FilterError};
pub use masking::{MaskLevel, MaskSite, Masker, Masking};
pub use model::{
    AgentMode, AgentSeen, BodyView, BrowserStatus, COLORS, Flow, FlowDetail, FlowSummary, McpCall,
    Message, RepeaterTab, SEVERITIES, Status, TimelineEntry,
};
pub use project::{Project, ProjectInfo};
pub use rules::{Rule, RuleMode, RulePart};
pub use scope::Scope;
pub use sender::{Edits, Target};
pub use sitemap::Endpoint;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// MCP log: what is stored of each request and response, and what lists show of them.
const MCP_LOG_MAX: usize = 256 * 1024;
const MCP_PREVIEW: usize = 300;

/// Cheap to clone; all clones share the same state.
#[derive(Clone)]
pub struct App {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    data_dir: PathBuf,
    started_at: u64,
    started: Instant,
    bus: events::Bus,
    /// The loaded project's database; swapped when the operator switches projects.
    store: RwLock<Arc<store::Store>>,
    project: Mutex<Project>,
    projects_dir: PathBuf,
    recent: project::Recent,
    ca: proxy::ca::Ca,
    crypto: Arc<rustls::crypto::CryptoProvider>,
    agent_mode: Mutex<AgentMode>,
    scope: Mutex<Scope>,
    /// The project's match & replace / alert rules, compiled for the proxy.
    rules: RwLock<Arc<rules::Rules>>,
    /// Agent name -> last time it sent something (unix ms).
    agents: Mutex<std::collections::HashMap<String, u64>>,
    /// Agent name -> the newest operator message it has been shown.
    agents_read: Mutex<std::collections::HashMap<String, String>>,
    /// Agent name -> its current intent (latest log_intent id); its requests carry it.
    intents: Mutex<std::collections::HashMap<String, String>>,
    /// Agent name -> the AI mode it was last told about.
    mode_seen: Mutex<std::collections::HashMap<String, AgentMode>>,
    proxy_running: AtomicBool,
    proxy_error: Mutex<Option<String>>,
    /// Open client connections on the proxy port, and its latest request (unix ms).
    proxy_connections: AtomicU64,
    proxy_last_request: AtomicU64,
    /// The MCP listener is served by the server crate; it reports its state here.
    mcp_running: AtomicBool,
    mcp_error: Mutex<Option<String>>,
    /// Required from non-local proxy clients when set (the proxy listens publicly).
    proxy_password: Mutex<Option<String>>,
    /// The AI browser while it runs; `browser_error` says why it could not open.
    browser: Mutex<Option<OpenBrowser>>,
    browser_error: Mutex<Option<String>>,
    /// The agent driving the AI browser and until when (unix ms; `u64::MAX` while its
    /// tool call runs). Requests the browser sends meanwhile are the agent's.
    browser_driver: Mutex<Option<(String, u64)>>,
    /// The project's masking settings and their ready translation.
    masking: RwLock<(Masking, Arc<Masker>)>,
}

/// A running AI browser: what to show, and how to close it.
struct OpenBrowser {
    binary: String,
    proxy_port: u16,
    cdp_port: u16,
    close: tokio::sync::mpsc::Sender<()>,
}

impl App {
    /// Opens the most recently used project (or creates "default" when there is none).
    pub fn open(config: Config, data_dir: PathBuf) -> io::Result<Self> {
        let projects_dir = config
            .projects_dir
            .clone()
            .unwrap_or_else(|| data_dir.join("projects"));
        fs::create_dir_all(&projects_dir)?;
        let recent = project::Recent(data_dir.join("recent.json"));
        let project = match recent
            .read()
            .iter()
            .find_map(|p| project::load(Path::new(p)).ok())
            .or_else(|| project::discover(&projects_dir, &[]).into_iter().next())
        {
            Some(p) => p,
            None => project::create(&projects_dir, "default")?,
        };
        recent.record(&project.path);
        let bus = events::Bus::new();
        let store = open_store(&project, &bus)?;
        let crypto = Arc::new(rustls::crypto::ring::default_provider());
        let ca = proxy::ca::Ca::load_or_create(&data_dir.join("ca"), crypto.clone())?;
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let inner = Inner {
            config,
            data_dir,
            started_at,
            started: Instant::now(),
            bus,
            store: RwLock::new(Arc::new(store)),
            project: Mutex::new(project),
            projects_dir,
            recent,
            ca,
            crypto,
            agent_mode: Mutex::new(AgentMode::Act),
            scope: Mutex::new(Scope::default()),
            rules: RwLock::default(),
            agents: Mutex::default(),
            agents_read: Mutex::default(),
            intents: Mutex::default(),
            mode_seen: Mutex::default(),
            proxy_running: AtomicBool::new(false),
            proxy_error: Mutex::new(None),
            proxy_connections: AtomicU64::new(0),
            proxy_last_request: AtomicU64::new(0),
            mcp_running: AtomicBool::new(false),
            mcp_error: Mutex::new(None),
            proxy_password: Mutex::new(None),
            browser: Mutex::default(),
            browser_error: Mutex::default(),
            browser_driver: Mutex::default(),
            masking: RwLock::new((
                Masking::default(),
                Arc::new(Masker::new(&Masking::default())),
            )),
        };
        let app = Self {
            inner: Arc::new(inner),
        };
        app.load_scope()?;
        Ok(app)
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    fn store(&self) -> Arc<store::Store> {
        self.inner.store.read().expect("store lock").clone()
    }

    /// The loaded project.
    pub fn project(&self) -> Project {
        self.inner.project.lock().expect("project lock").clone()
    }

    /// Where new projects go unless a folder is given.
    pub fn projects_dir(&self) -> &Path {
        &self.inner.projects_dir
    }

    /// All known projects: the projects dir plus recently opened folders elsewhere.
    pub fn projects(&self) -> Vec<ProjectInfo> {
        let current = self.project();
        project::discover(&self.inner.projects_dir, &self.inner.recent.read())
            .into_iter()
            .map(|p| {
                let folder = Path::new(&p.path);
                let loaded = p.path == current.path;
                ProjectInfo {
                    flows: if loaded {
                        Some(self.store().count())
                    } else {
                        store::count_in(&folder.join(project::DB))
                    },
                    size: project::size(folder),
                    loaded,
                    project: p,
                }
            })
            .collect()
    }

    /// Creates a project (in `folder`, or the projects dir), switches to it and sets its scope.
    pub fn create_project(
        &self,
        name: &str,
        folder: Option<&Path>,
        scope: Scope,
    ) -> io::Result<Project> {
        let base = folder.unwrap_or(&self.inner.projects_dir);
        let project = project::create(base, name)?;
        self.switch_project(Path::new(&project.path))?;
        self.set_scope(scope)?;
        Ok(project)
    }

    /// Loads the project in `folder`. Captures from now on go there; every
    /// window is told to reload.
    pub fn switch_project(&self, folder: &Path) -> io::Result<Project> {
        let project = project::load(folder)?;
        if project.path != self.project().path {
            let store = Arc::new(open_store(&project, &self.inner.bus)?);
            let old = std::mem::replace(&mut *self.inner.store.write().expect("store lock"), store);
            old.close();
            *self.inner.project.lock().expect("project lock") = project.clone();
            self.inner
                .agents_read
                .lock()
                .expect("agents_read lock")
                .clear();
            self.load_scope()?;
            self.close_browser(); // its profile belongs to the old project
        }
        self.inner.recent.record(&project.path);
        self.inner.bus.emit(Event::ProjectChanged {
            project: project.clone(),
        });
        Ok(project)
    }

    /// Deletes a project folder with everything captured in it. The loaded
    /// project cannot be deleted: switch to another one first.
    pub fn delete_project(&self, folder: &Path) -> io::Result<()> {
        let project = project::load(folder)?;
        if project.path == self.project().path {
            return Err(invalid(
                "this project is loaded; switch to another one before deleting it".into(),
            ));
        }
        project::delete(Path::new(&project.path))?;
        self.inner.recent.forget(&project.path);
        Ok(())
    }

    pub fn status(&self) -> Status {
        let i = &self.inner;
        let agent_list = self.active_agents();
        Status {
            version: VERSION.into(),
            pid: std::process::id(),
            started_at: i.started_at,
            uptime_s: i.started.elapsed().as_secs(),
            project: self.project().name,
            project_path: self.project().path,
            data_dir: i.data_dir.display().to_string(),
            api_addr: i.config.api_addr.to_string(),
            proxy_addr: i.config.proxy_addr.to_string(),
            proxy_running: i.proxy_running.load(Ordering::Relaxed),
            proxy_error: i.proxy_error.lock().expect("proxy_error lock").clone(),
            proxy_connections: i.proxy_connections.load(Ordering::Relaxed),
            proxy_last_request: i.proxy_last_request.load(Ordering::Relaxed),
            mcp_addr: i.config.mcp_addr.to_string(),
            mcp_running: i.mcp_running.load(Ordering::Relaxed),
            mcp_error: i.mcp_error.lock().expect("mcp_error lock").clone(),
            ca_fingerprint: i.ca.fingerprint().to_string(),
            flows: self.store().count(),
            dropped: self.store().dropped(),
            agents: agent_list.len() as u32,
            agent_list,
            ai_paused: self.ai_paused(),
            agent_mode: self.agent_mode(),
            browser: self.browser(),
            masking: self.masking(),
        }
    }

    /// Live stream of everything that happens (new flows, tags, AI notes, ...).
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.bus.subscribe()
    }

    /// The newest `limit` flows, oldest first.
    pub fn recent_flows(&self, limit: usize) -> io::Result<Vec<FlowSummary>> {
        self.query_flows(&Filter::default(), limit)
    }

    /// The newest `limit` flows matching `filter`, oldest first.
    /// A page of history: the newest `limit` matches older than `before` (if given), oldest first.
    pub fn query_page(
        &self,
        filter: &Filter,
        limit: usize,
        before: Option<&str>,
    ) -> io::Result<Vec<FlowSummary>> {
        self.store()
            .query_page(filter, limit, before)
            .map_err(io::Error::other)
    }

    pub fn count_flows(&self, filter: &Filter) -> io::Result<u64> {
        self.store()
            .count_matching(filter)
            .map_err(io::Error::other)
    }

    pub fn query_flows(&self, filter: &Filter, limit: usize) -> io::Result<Vec<FlowSummary>> {
        self.store().query(filter, limit).map_err(io::Error::other)
    }

    /// Adds and removes tags on flows. `actor` is "human" or the agent's name.
    pub fn tag_flows(
        &self,
        ids: &[String],
        add: &[String],
        remove: &[String],
        actor: &str,
    ) -> io::Result<()> {
        let clean = |tags: &[String]| -> io::Result<Vec<String>> {
            tags.iter().map(|t| valid_tag(t)).collect()
        };
        let (add, remove) = (clean(add)?, clean(remove)?);
        self.store()
            .tag(ids, &add, &remove, actor)
            .map_err(io::Error::other)?;
        self.emit_updated(ids)
    }

    /// Highlights flows with one of [`COLORS`], or clears it with `None`.
    pub fn color_flows(&self, ids: &[String], color: Option<&str>) -> io::Result<()> {
        if let Some(c) = color.filter(|c| !COLORS.contains(c)) {
            return Err(invalid(format!(
                "unknown color {c:?} (use {})",
                COLORS.join(", ")
            )));
        }
        self.store()
            .set_field(ids, "color", color)
            .map_err(io::Error::other)?;
        self.emit_updated(ids)
    }

    /// Sets a note on flows; an empty note clears it.
    pub fn note_flows(&self, ids: &[String], note: &str) -> io::Result<()> {
        let note = note.trim();
        if note.len() > 10_000 {
            return Err(invalid("notes are limited to 10,000 characters".into()));
        }
        let value = (!note.is_empty()).then_some(note);
        self.store()
            .set_field(ids, "note", value)
            .map_err(io::Error::other)?;
        self.emit_updated(ids)
    }

    /// Editor text for a captured request, ready for the Repeater.
    pub fn flow_raw(&self, id: &str) -> io::Result<Option<String>> {
        self.flow(id)?
            .map(|f| sender::raw_from_flow(&f))
            .transpose()
    }

    // ---- Repeater ------------------------------------------------------------

    pub fn repeater_tabs(&self) -> io::Result<Vec<RepeaterTab>> {
        self.store().repeater_tabs().map_err(io::Error::other)
    }

    /// Opens a tab with a copy of a captured request, or a blank template.
    pub fn repeater_open(&self, from_flow: Option<&str>) -> io::Result<RepeaterTab> {
        let id = crate::new_id();
        let tab = match from_flow {
            Some(flow_id) => {
                let flow = self
                    .flow(flow_id)?
                    .ok_or_else(|| invalid(format!("no flow {flow_id}")))?;
                RepeaterTab {
                    id,
                    name: format!("{} {}", flow.method, flow.path),
                    target: Target {
                        scheme: flow.scheme.clone(),
                        host: flow.host.clone(),
                        port: flow.port,
                    },
                    raw: sender::raw_from_flow(&flow)?,
                    sends: Vec::new(),
                }
            }
            None => RepeaterTab {
                id,
                name: "New request".into(),
                target: Target {
                    scheme: "https".into(),
                    host: "example.com".into(),
                    port: 443,
                },
                raw: "GET / HTTP/1.1\nHost: example.com\n\n".into(),
                sends: Vec::new(),
            },
        };
        self.save_tab(&tab)?;
        Ok(tab)
    }

    /// Saves edits (name, target, request text) to an open tab.
    pub fn repeater_update(
        &self,
        id: &str,
        name: Option<String>,
        target: Option<Target>,
        raw: Option<String>,
    ) -> io::Result<RepeaterTab> {
        let mut tab = self.tab(id)?;
        if let Some(target) = target {
            target.validate()?;
            tab.target = target;
        }
        if let Some(name) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
            tab.name = name;
        }
        if let Some(raw) = raw {
            tab.raw = editor_text(&raw);
        }
        self.save_tab(&tab)?;
        Ok(tab)
    }

    pub fn repeater_close(&self, id: &str) -> io::Result<()> {
        self.store()
            .delete_repeater_tab(id)
            .map_err(io::Error::other)?;
        self.emit_tabs()
    }

    /// Sends the tab's request and remembers the result in the tab's history.
    /// Sends the tab's request; with `follow`, also up to 10 redirects. Every step is
    /// recorded in History; the result is the last response and how many redirects led there.
    pub async fn repeater_send(
        &self,
        id: &str,
        actor: &str,
        follow: bool,
    ) -> io::Result<(FlowDetail, usize)> {
        const MAX_REDIRECTS: usize = 10;
        let tab = self.tab(id)?;
        let (mut target, mut raw) = (tab.target.clone(), tab.raw.clone());
        let mut flow = sender::send(self, &target, &raw, actor, "repeater", None).await?;
        let mut redirects = 0;
        while follow && redirects < MAX_REDIRECTS {
            let Some((next, next_raw)) = sender::redirect(&target, &raw, &flow) else {
                break;
            };
            flow = sender::send(self, &next, &next_raw, actor, "repeater", None).await?;
            (target, raw) = (next, next_raw);
            redirects += 1;
        }
        let mut tab = self.tab(id)?; // re-read: it may have been edited meanwhile
        tab.sends.push(flow.id.clone());
        self.save_tab(&tab)?;
        Ok((FlowDetail::from(flow), redirects))
    }

    fn tab(&self, id: &str) -> io::Result<RepeaterTab> {
        self.repeater_tabs()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| invalid(format!("no repeater tab {id}")))
    }

    fn save_tab(&self, tab: &RepeaterTab) -> io::Result<()> {
        self.store()
            .save_repeater_tab(tab)
            .map_err(io::Error::other)?;
        self.emit_tabs()
    }

    fn emit_tabs(&self) -> io::Result<()> {
        let tabs = self.repeater_tabs()?;
        self.inner.bus.emit(Event::RepeaterChanged { tabs });
        Ok(())
    }

    fn emit_updated(&self, ids: &[String]) -> io::Result<()> {
        for flow in self.store().summaries(ids).map_err(io::Error::other)? {
            self.inner.bus.emit(Event::FlowUpdated {
                flow: Box::new(flow),
            });
        }
        Ok(())
    }

    pub fn flow(&self, id: &str) -> io::Result<Option<Flow>> {
        self.store().get(id).map_err(io::Error::other)
    }

    /// One flow prepared for display (decoded bodies, text or base64).
    pub fn flow_detail(&self, id: &str) -> io::Result<Option<FlowDetail>> {
        Ok(self.flow(id)?.map(FlowDetail::from))
    }

    /// The CA certificate clients must trust to have HTTPS intercepted (PEM).
    pub fn ca_cert_pem(&self) -> &str {
        self.inner.ca.cert_pem()
    }

    /// Runs the intercepting proxy on `listener` until `stop` turns true.
    pub async fn serve_proxy(&self, listener: TcpListener, stop: watch::Receiver<bool>) {
        self.set_proxy_error(None);
        self.inner.proxy_running.store(true, Ordering::Relaxed);
        proxy::serve(self.clone(), listener, stop).await;
        self.inner.proxy_running.store(false, Ordering::Relaxed);
    }

    /// Clients from other machines must send this password (`http://name:PASSWORD@…`).
    pub fn set_proxy_password(&self, password: Option<String>) {
        *self.inner.proxy_password.lock().expect("password lock") = password;
    }

    pub(crate) fn proxy_password_ok(&self, given: Option<&str>) -> bool {
        match self
            .inner
            .proxy_password
            .lock()
            .expect("password lock")
            .as_deref()
        {
            None => true,
            Some(want) => given.is_some_and(|g| {
                g.len() == want.len()
                    && g.bytes()
                        .zip(want.bytes())
                        .fold(0, |acc, (a, b)| acc | (a ^ b))
                        == 0
            }),
        }
    }

    /// Shown in status, e.g. when the proxy port is already taken.
    pub fn set_proxy_error(&self, error: Option<String>) {
        *self.inner.proxy_error.lock().expect("proxy_error lock") = error;
    }

    /// The server reports whether the MCP listener runs, or why not (e.g. port in use).
    pub fn set_mcp_state(&self, running: bool, error: Option<String>) {
        self.inner.mcp_running.store(running, Ordering::Relaxed);
        *self.inner.mcp_error.lock().expect("mcp_error lock") = error;
    }

    /// Counts a proxy client connection until the returned guard is dropped.
    pub(crate) fn proxy_connection(&self) -> ConnectionGuard {
        self.inner.proxy_connections.fetch_add(1, Ordering::Relaxed);
        ConnectionGuard(self.clone())
    }

    // ---- Agents: scope, kill switch, attribution ------------------------------

    pub fn scope(&self) -> Scope {
        self.inner.scope.lock().expect("scope lock").clone()
    }

    pub fn set_scope(&self, scope: Scope) -> io::Result<Scope> {
        let scope = scope.normalized()?;
        let store = &self.store();
        store
            .set_setting("scope.include", &scope.include.join("\n"))
            .map_err(io::Error::other)?;
        store
            .set_setting("scope.exclude", &scope.exclude.join("\n"))
            .map_err(io::Error::other)?;
        *self.inner.scope.lock().expect("scope lock") = scope.clone();
        let masker = self.masker();
        for host in &scope.include {
            masker.learn(host); // Strict: new scope hosts get aliases right away
        }
        self.keep_mask_aliases();
        self.inner.bus.emit(Event::ScopeChanged {
            scope: scope.clone(),
        });
        Ok(scope)
    }

    fn load_scope(&self) -> io::Result<()> {
        let read = |key| -> io::Result<Vec<String>> {
            let value = self
                .store()
                .setting(key)
                .map_err(io::Error::other)?
                .unwrap_or_default();
            Ok(value.lines().map(str::to_string).collect())
        };
        let scope = Scope {
            include: read("scope.include")?,
            exclude: read("scope.exclude")?,
        };
        *self.inner.scope.lock().expect("scope lock") = scope;
        let masking = match self.store().setting("masking").map_err(io::Error::other)? {
            Some(json) => serde_json::from_str(&json).unwrap_or_default(),
            None => Masking::default(),
        };
        self.use_masking(masking);
        let mode = match self
            .store()
            .setting("agents.mode")
            .map_err(io::Error::other)?
            .as_deref()
        {
            Some("advise") => AgentMode::Advise,
            Some("off") => AgentMode::Off,
            _ => AgentMode::Act,
        };
        *self.inner.agent_mode.lock().expect("mode lock") = mode;
        let compiled = rules::Rules::compile(&self.rules()).unwrap_or_default();
        *self.inner.rules.write().expect("rules lock") = Arc::new(compiled);
        Ok(())
    }

    /// The project's match & replace / alert rules, in order.
    pub fn rules(&self) -> Vec<Rule> {
        self.store()
            .setting("rules")
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    /// Replaces all rules (checked first: a bad pattern changes nothing).
    pub fn set_rules(&self, mut list: Vec<Rule>) -> io::Result<Vec<Rule>> {
        for rule in &mut list {
            rule.name = rule.name.trim().to_string();
            if rule.id.is_empty() {
                rule.id = new_id();
            }
        }
        let compiled = rules::Rules::compile(&list).map_err(invalid)?;
        let json = serde_json::to_string(&list).map_err(io::Error::other)?;
        self.store()
            .set_setting("rules", &json)
            .map_err(io::Error::other)?;
        *self.inner.rules.write().expect("rules lock") = Arc::new(compiled);
        Ok(list)
    }

    pub(crate) fn compiled_rules(&self) -> Arc<rules::Rules> {
        self.inner.rules.read().expect("rules lock").clone()
    }

    /// An alert rule fired on a request passing the proxy.
    pub(crate) fn rule_alert(&self, rule: &str, flow: &Flow, blocked: bool) {
        self.inner.bus.emit(Event::RuleAlert {
            rule: rule.to_string(),
            blocked,
            flow_id: flow.id.clone(),
            seq: flow.seq,
            method: flow.method.clone(),
            url: format!("{}://{}{}", flow.scheme, flow.host, flow.path),
        });
    }

    /// May `actor` send to `host`? The operator's own traffic always may; agents
    /// only while not paused and only inside the scope.
    pub fn agent_allowed(&self, actor: &str, host: &str) -> Result<(), String> {
        if actor == "proxy" || actor == "human" {
            return Ok(());
        }
        match self.agent_mode() {
            AgentMode::Off => return Err("blocked: the operator has paused all AI agents".into()),
            AgentMode::Advise => {
                return Err(
                    "blocked: shiftWire is in advise mode, so agents do not send requests themselves. \
                     Use prepare_request: the operator reviews it in the Repeater and sends it."
                        .into(),
                );
            }
            AgentMode::Act => {}
        }
        let scope = self.scope();
        if !scope.allows(host) {
            return Err(format!(
                "blocked: {host} is out of scope (scope {})",
                scope.describe()
            ));
        }
        Ok(())
    }

    /// Remembers that an agent was active (for "agents" in status).
    pub fn touch_agent(&self, name: &str) {
        self.inner
            .agents
            .lock()
            .expect("agents lock")
            .insert(name.to_string(), now_ms());
    }

    /// Agents active in the last 10 minutes, most recent first.
    fn active_agents(&self) -> Vec<AgentSeen> {
        let since = now_ms().saturating_sub(10 * 60 * 1000);
        let mut list: Vec<AgentSeen> = self
            .inner
            .agents
            .lock()
            .expect("agents lock")
            .iter()
            .filter(|&(_, &t)| t >= since)
            .map(|(name, &last_seen)| AgentSeen {
                name: name.clone(),
                last_seen,
            })
            .collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.last_seen));
        list
    }

    /// An agent's request (MCP `send_request`): refused requests are recorded too.
    pub async fn agent_send(
        &self,
        agent: &str,
        target: &Target,
        raw: &str,
    ) -> io::Result<FlowDetail> {
        self.touch_agent(agent);
        let blocked = self.agent_allowed(agent, &target.host).err();
        let flow = sender::send(self, target, raw, agent, "mcp", blocked).await?;
        Ok(FlowDetail::from(flow))
    }

    /// Resends a captured request with changes (MCP `repeat_flow`).
    pub async fn agent_repeat(
        &self,
        agent: &str,
        id: &str,
        edits: &Edits,
    ) -> io::Result<FlowDetail> {
        let flow = self
            .flow(id)?
            .ok_or_else(|| invalid(format!("no flow {id}")))?;
        let raw = sender::apply_edits(&sender::raw_from_flow(&flow)?, edits)?;
        let target = Target {
            scheme: flow.scheme,
            host: flow.host,
            port: flow.port,
        };
        self.agent_send(agent, &target, &raw).await
    }

    /// Every endpoint seen (optionally for one host), IDs collapsed into patterns.
    pub fn sitemap(&self, host: Option<&str>) -> io::Result<Vec<Endpoint>> {
        let groups = self.store().site_groups(host).map_err(io::Error::other)?;
        let scope = self.scope();
        let mut endpoints = sitemap::build(groups);
        for e in &mut endpoints {
            e.in_scope = scope.allows(&e.host);
        }
        Ok(endpoints)
    }

    // ---- AI Timeline ------------------------------------------------------------

    /// The newest `limit` timeline entries, oldest first.
    pub fn timeline(&self, limit: usize) -> io::Result<Vec<TimelineEntry>> {
        self.store()
            .timeline(limit, None, None)
            .map_err(io::Error::other)
    }

    /// An agent says what it is about to do; its requests that follow belong to it.
    /// Records what an agent is about to do. Its requests from now on carry the
    /// entry's id (filter `intent:<id>`), until its next intent.
    pub fn log_intent(&self, actor: &str, text: &str) -> io::Result<TimelineEntry> {
        let entry = self.add_timeline(actor, "intent", text, None, None)?;
        self.inner
            .intents
            .lock()
            .expect("intents lock")
            .insert(actor.to_string(), entry.id.clone());
        Ok(entry)
    }

    /// The intent an agent's requests are sent under right now.
    pub(crate) fn current_intent(&self, actor: &str) -> Option<String> {
        self.inner
            .intents
            .lock()
            .expect("intents lock")
            .get(actor)
            .cloned()
    }

    /// Ids of the flows matching `filter` (at most `limit`), for bulk tags and notes.
    pub fn flow_ids(&self, filter: &Filter, limit: usize) -> io::Result<Vec<String>> {
        self.store().ids(filter, limit).map_err(io::Error::other)
    }

    /// The AI mode, if it changed since `agent` last heard about it (the first call
    /// only remembers it): so agents learn about a switch with their next action.
    pub fn mode_news(&self, agent: &str) -> Option<AgentMode> {
        let mode = self.agent_mode();
        let mut seen = self.inner.mode_seen.lock().expect("mode_seen lock");
        match seen.insert(agent.to_string(), mode) {
            Some(before) if before != mode => Some(mode),
            _ => None,
        }
    }

    /// Flags a request as important, with a severity and a reason the operator reads.
    pub fn mark_flow(
        &self,
        actor: &str,
        flow_id: &str,
        severity: &str,
        reason: &str,
    ) -> io::Result<TimelineEntry> {
        let severity = severity.trim().to_ascii_lowercase();
        if !SEVERITIES.contains(&severity.as_str()) {
            return Err(invalid(format!(
                "unknown severity {severity:?} (use {})",
                SEVERITIES.join(", ")
            )));
        }
        let entry = self.add_timeline(
            actor,
            "mark",
            reason,
            Some(flow_id.to_string()),
            Some(severity),
        )?;
        self.emit_updated(&[flow_id.to_string()])?;
        Ok(entry)
    }

    /// A message from the operator to all agents (they get it with their next tool call).
    pub fn message_agents(&self, text: &str) -> io::Result<TimelineEntry> {
        self.add_timeline("human", "operator", text, None, None)
    }

    /// Operator messages this agent has not seen yet (new agents: the last 30 minutes).
    pub fn unread_operator_messages(&self, agent: &str) -> io::Result<Vec<TimelineEntry>> {
        let last = self
            .inner
            .agents_read
            .lock()
            .expect("read lock")
            .get(agent)
            .cloned();
        let after = last.unwrap_or_else(|| {
            let since = now_ms().saturating_sub(30 * 60 * 1000);
            ulid::Ulid::from_parts(since, 0).to_string()
        });
        let messages = self
            .store()
            .timeline(50, Some("operator"), Some(&after))
            .map_err(io::Error::other)?;
        if let Some(newest) = messages.last() {
            self.inner
                .agents_read
                .lock()
                .expect("read lock")
                .insert(agent.to_string(), newest.id.clone());
        }
        Ok(messages)
    }

    fn add_timeline(
        &self,
        actor: &str,
        kind: &str,
        text: &str,
        flow_id: Option<String>,
        severity: Option<String>,
    ) -> io::Result<TimelineEntry> {
        let text = text.trim();
        if text.is_empty() || text.len() > 4000 {
            return Err(invalid("the text must be 1 to 4,000 characters".into()));
        }
        let entry = TimelineEntry {
            id: crate::new_id(),
            ts: now_ms(),
            actor: actor.to_string(),
            kind: kind.to_string(),
            text: text.to_string(),
            flow_id,
            severity,
        };
        self.store()
            .add_timeline(&entry)
            .map_err(io::Error::other)?;
        self.inner.bus.emit(Event::Timeline {
            entry: entry.clone(),
        });
        Ok(entry)
    }

    // ---- Masking ------------------------------------------------------------------

    /// The project's masking settings (operator only; agents never see real names here).
    pub fn masking(&self) -> Masking {
        let (masking, masker) = &*self.inner.masking.read().expect("masking lock");
        Masking {
            auto: masker.auto_sites(), // Strict: grows as agents meet other hosts
            ..masking.clone()
        }
    }

    /// What MCP uses to translate for agents.
    pub fn masker(&self) -> Arc<Masker> {
        self.inner.masking.read().expect("masking lock").1.clone()
    }

    pub fn set_masking(&self, masking: Masking) -> io::Result<Masking> {
        self.use_masking(masking.normalized()?);
        self.save_masking()?;
        Ok(self.masking())
    }

    /// Strict gives other hosts aliases while it masks; MCP calls this afterwards so
    /// they stay the same next time.
    pub fn keep_mask_aliases(&self) {
        if self.masker().take_new_aliases()
            && let Err(e) = self.save_masking()
        {
            tracing::warn!("cannot save masking aliases: {e}");
        }
    }

    /// Makes `masking` the active one; Strict gives the scope's hosts aliases at once.
    fn use_masking(&self, masking: Masking) {
        let masker = Arc::new(Masker::new(&masking));
        for host in &self.scope().include {
            masker.learn(host);
        }
        *self.inner.masking.write().expect("masking lock") = (masking, masker);
    }

    fn save_masking(&self) -> io::Result<()> {
        let json = serde_json::to_string(&self.masking()).map_err(io::Error::other)?;
        self.store()
            .set_setting("masking", &json)
            .map_err(io::Error::other)
    }

    // ---- AI browser ---------------------------------------------------------------

    pub fn browser(&self) -> BrowserStatus {
        let open = self.inner.browser.lock().expect("browser lock");
        BrowserStatus {
            running: open.is_some(),
            binary: open.as_ref().map(|b| b.binary.clone()),
            proxy_port: open.as_ref().map(|b| b.proxy_port),
            cdp_port: open.as_ref().map(|b| b.cdp_port),
            error: self
                .inner
                .browser_error
                .lock()
                .expect("browser lock")
                .clone(),
        }
    }

    /// Starts the AI browser (nothing happens if it is open): its own proxy listener
    /// on a free loopback port, then the browser with the project's profile. When the
    /// browser exits, its listener closes too.
    pub async fn open_browser(&self) -> io::Result<BrowserStatus> {
        if self.inner.browser.lock().expect("browser lock").is_some() {
            return Ok(self.browser());
        }
        let result = self.start_browser().await;
        *self.inner.browser_error.lock().expect("browser lock") =
            result.as_ref().err().map(|e| e.to_string());
        result.map(|()| self.browser())
    }

    async fn start_browser(&self) -> io::Result<()> {
        let binary = browser::find(self.config().browser_path.as_deref()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no Chrome, Edge or Chromium found: install one, or set browser_path in config.toml",
            )
        })?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let proxy_port = listener.local_addr()?.port();
        let cdp_port = std::net::TcpListener::bind(("127.0.0.1", 0))?
            .local_addr()?
            .port(); // free right now; the browser takes it a moment later
        let profile = Path::new(&self.project().path).join("browser");
        let args = browser::args(proxy_port, cdp_port, &profile, self.inner.ca.spki_hashes());
        let mut child = tokio::process::Command::new(&binary)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                io::Error::new(e.kind(), format!("cannot start {}: {e}", binary.display()))
            })?;
        let (stop, stop_rx) = watch::channel(false);
        tokio::spawn(proxy::serve_browser(self.clone(), listener, stop_rx));
        let (close, mut close_rx) = tokio::sync::mpsc::channel(1);
        *self.inner.browser.lock().expect("browser lock") = Some(OpenBrowser {
            binary: binary.display().to_string(),
            proxy_port,
            cdp_port,
            close,
        });
        let app = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {}
                _ = close_rx.recv() => drop(child.kill().await),
            }
            stop.send_replace(true);
            let mut open = app.inner.browser.lock().expect("browser lock");
            if open.as_ref().is_some_and(|b| b.proxy_port == proxy_port) {
                *open = None;
            }
        });
        Ok(())
    }

    /// An agent's browser tool call starts: what the browser requests now is the agent's.
    pub fn begin_browser_action(&self, agent: &str) {
        *self.inner.browser_driver.lock().expect("driver lock") =
            Some((agent.to_string(), u64::MAX));
    }

    /// The call returned; requests it set off may still follow for a second.
    pub fn end_browser_action(&self, agent: &str) {
        let mut driver = self.inner.browser_driver.lock().expect("driver lock");
        if let Some((name, until)) = driver.as_mut()
            && name == agent
        {
            *until = now_ms() + 1000;
        }
    }

    /// The agent the AI browser's requests belong to right now, if any (else the operator's).
    pub(crate) fn browser_actor(&self) -> Option<String> {
        let driver = self.inner.browser_driver.lock().expect("driver lock");
        driver
            .as_ref()
            .filter(|(_, until)| now_ms() <= *until)
            .map(|(name, _)| name.clone())
    }

    /// Closes the AI browser, if it is open.
    pub fn close_browser(&self) {
        if let Some(open) = self.inner.browser.lock().expect("browser lock").as_ref() {
            let _ = open.close.try_send(());
        }
    }

    // ---- MCP log ------------------------------------------------------------------

    /// Keeps an agent's MCP request and the reply it got. Stored up to [`MCP_LOG_MAX`]
    /// characters each; the UI is told with a preview.
    /// Assigns the call's `id` and `ts` (its start).
    pub fn record_mcp_call(&self, mut call: McpCall) -> io::Result<()> {
        call.id = new_id();
        call.ts = now_ms().saturating_sub(call.duration_ms);
        for text in [&mut call.request, &mut call.response] {
            if text.len() > MCP_LOG_MAX {
                let cut = (0..=MCP_LOG_MAX)
                    .rev()
                    .find(|&i| text.is_char_boundary(i))
                    .unwrap_or(0);
                let total = text.len();
                text.truncate(cut);
                text.push_str(&format!("\n[… shiftWire kept {cut} of {total} bytes]"));
            }
        }
        // Sizes in characters, like the previews (and SQLite's length()).
        call.request_size = call.request.chars().count() as u64;
        call.response_size = call.response.chars().count() as u64;
        self.store().add_mcp_call(&call).map_err(io::Error::other)?;
        for text in [&mut call.request, &mut call.response] {
            *text = text.chars().take(MCP_PREVIEW).collect();
        }
        self.inner.bus.emit(Event::McpCall { call });
        Ok(())
    }

    /// The newest `limit` MCP calls older than `before`, oldest first, as previews.
    pub fn mcp_calls(&self, limit: usize, before: Option<&str>) -> io::Result<Vec<McpCall>> {
        self.store()
            .mcp_calls(limit, before, Some(MCP_PREVIEW), None)
            .map_err(io::Error::other)
    }

    /// Highlights MCP calls in the MCP log, like flows in History (`None` clears it).
    pub fn color_mcp_calls(&self, ids: &[String], color: Option<&str>) -> io::Result<()> {
        if let Some(c) = color.filter(|c| !COLORS.contains(c)) {
            return Err(invalid(format!(
                "unknown color {c:?} (use {})",
                COLORS.join(", ")
            )));
        }
        self.store()
            .color_mcp_calls(ids, color)
            .map_err(io::Error::other)?;
        self.inner.bus.emit(Event::McpCallsColored {
            ids: ids.to_vec(),
            color: color.map(str::to_string),
        });
        Ok(())
    }

    /// One MCP call in full.
    pub fn mcp_call(&self, id: &str) -> io::Result<Option<McpCall>> {
        let mut found = self
            .store()
            .mcp_calls(1, None, None, Some(id))
            .map_err(io::Error::other)?;
        Ok(found.pop())
    }

    /// Kill switch: while paused, every request from an AI agent is refused.
    pub fn agent_mode(&self) -> AgentMode {
        *self.inner.agent_mode.lock().expect("mode lock")
    }

    /// Chooses what agents may do; remembered across restarts.
    pub fn set_agent_mode(&self, mode: AgentMode) -> io::Result<()> {
        let changed =
            std::mem::replace(&mut *self.inner.agent_mode.lock().expect("mode lock"), mode) != mode;
        self.store()
            .set_setting("agents.mode", mode.as_str())
            .map_err(io::Error::other)?;
        if changed {
            self.inner.bus.emit(Event::AgentMode { mode });
        }
        Ok(())
    }

    pub fn ai_paused(&self) -> bool {
        self.agent_mode() == AgentMode::Off
    }

    /// Shorthand for the kill switch: `off` when paused, `act` otherwise.
    pub fn set_ai_paused(&self, paused: bool) {
        let _ = self.set_agent_mode(if paused {
            AgentMode::Off
        } else {
            AgentMode::Act
        });
    }

    /// An agent prepares a request for the operator: a new Repeater tab, not sent.
    pub fn prepare_request(
        &self,
        agent: &str,
        target: &Target,
        raw: &str,
        name: &str,
    ) -> io::Result<RepeaterTab> {
        target.validate()?;
        sender::check_raw(raw)?;
        let name = if name.trim().is_empty() {
            raw.lines().next().unwrap_or("request").to_string()
        } else {
            name.trim().to_string()
        };
        let tab = RepeaterTab {
            id: crate::new_id(),
            name: format!("{agent}: {name}"),
            target: target.clone(),
            raw: editor_text(raw),
            sends: Vec::new(),
        };
        self.save_tab(&tab)?;
        self.add_timeline(
            agent,
            "prepared",
            &format!("Prepared \"{name}\" in the Repeater for you to review and send."),
            None,
            None,
        )?;
        Ok(tab)
    }

    /// Flushes pending captures to disk. Call once on shutdown.
    pub fn close(&self) {
        self.close_browser();
        self.store().close();
    }

    /// The `#` for a request that is starting now.
    pub(crate) fn next_seq(&self) -> u64 {
        self.store().next_seq()
    }

    pub(crate) fn record(&self, flow: Flow) {
        if matches!(flow.source.as_str(), "proxy" | "tunnel") {
            self.inner
                .proxy_last_request
                .fetch_max(flow.ts_start, Ordering::Relaxed);
        }
        self.store().record(flow);
    }

    pub(crate) fn ca(&self) -> &proxy::ca::Ca {
        &self.inner.ca
    }

    pub(crate) fn crypto(&self) -> Arc<rustls::crypto::CryptoProvider> {
        self.inner.crypto.clone()
    }
}

/// Repeater text uses `\n` line endings (as the editor shows it); agents often send
/// `\r\n`. The sender writes proper CRLF on the wire either way.
fn editor_text(raw: &str) -> String {
    raw.replace("\r\n", "\n")
}

/// See [`App::proxy_connection`].
pub(crate) struct ConnectionGuard(App);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let open = &self.0.inner.proxy_connections;
        open.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A new ULID. Monotonic: ids made in the same millisecond still sort in
/// creation order, so flows and timeline entries never swap places.
pub(crate) fn new_id() -> String {
    static GENERATOR: Mutex<Option<ulid::Generator>> = Mutex::new(None);
    let mut generator = GENERATOR.lock().expect("id generator lock");
    let generator = generator.get_or_insert_with(ulid::Generator::new);
    generator
        .generate()
        .unwrap_or_else(|_| ulid::Ulid::new())
        .to_string()
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn valid_tag(tag: &str) -> io::Result<String> {
    let tag = tag.trim();
    if tag.is_empty() || tag.len() > 64 || tag.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(invalid(format!(
            "invalid tag {tag:?}: 1-64 characters, no spaces"
        )));
    }
    Ok(tag.to_string())
}

fn open_store(project: &Project, bus: &events::Bus) -> io::Result<store::Store> {
    let db = Path::new(&project.path).join(project::DB);
    store::Store::open(&db, bus.clone()).map_err(io::Error::other)
}

#[cfg(test)]
pub(crate) fn test_app(name: &str) -> App {
    let dir = std::env::temp_dir().join(format!("shiftwire-core-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    App::open(Config::default(), dir).expect("open test app")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reflects_config() {
        let app = test_app("status");
        let s = app.status();
        assert_eq!(s.project, "default");
        assert_eq!(s.api_addr, "127.0.0.1:7777");
        assert_eq!(s.version, VERSION);
        assert_eq!(s.flows, 0);
    }

    #[test]
    fn projects_switch_live_and_keep_their_own_data() {
        let app = test_app("projects");
        let first = app.project();
        app.log_intent("claude", "in the first project").unwrap();
        let mut events = app.subscribe();

        let scope = Scope {
            include: vec!["shop.test".into()],
            exclude: vec![],
        };
        let second = app.create_project("Shop", None, scope).unwrap();
        assert!(matches!(
            events.try_recv(),
            Ok(Event::ProjectChanged { .. })
        ));
        assert_eq!(app.status().project, "Shop");
        assert!(
            app.timeline(10).unwrap().is_empty(),
            "a new project starts empty"
        );
        assert!(app.agent_allowed("claude", "shop.test").is_ok());

        app.switch_project(Path::new(&first.path)).unwrap();
        assert_eq!(app.timeline(10).unwrap().len(), 1);
        assert!(
            app.agent_allowed("claude", "shop.test").is_err(),
            "scope belongs to the project"
        );
        assert_eq!(app.projects().len(), 2);

        assert!(
            app.delete_project(Path::new(&first.path)).is_err(),
            "loaded"
        );
        app.delete_project(Path::new(&second.path)).unwrap();
        assert_eq!(app.projects().len(), 1);

        // A restart opens the project used last.
        let reopened = App::open(Config::default(), app.inner.data_dir.clone()).unwrap();
        assert_eq!(reopened.project(), first);
    }

    #[test]
    fn timeline_marks_and_operator_messages() {
        let app = test_app("timeline");
        app.log_intent("claude", "Testing IDOR on /api/users/{id}")
            .unwrap();
        assert!(
            app.mark_flow("claude", "f1", "urgent", "x").is_err(),
            "unknown severity"
        );
        assert!(app.log_intent("claude", "   ").is_err(), "empty text");
        app.mark_flow("claude", "f1", "HIGH", "user 7 can read user 8")
            .unwrap();
        let kinds: Vec<_> = app
            .timeline(10)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds, ["intent", "mark"]);

        app.message_agents("stay away from /admin").unwrap();
        assert_eq!(app.unread_operator_messages("claude").unwrap().len(), 1);
        assert!(
            app.unread_operator_messages("claude").unwrap().is_empty(),
            "delivered once"
        );
        assert_eq!(
            app.unread_operator_messages("other").unwrap().len(),
            1,
            "each agent gets it"
        );
    }

    #[test]
    fn advise_mode_lets_agents_prepare_but_not_send() {
        let app = test_app("advise");
        app.set_scope(Scope {
            include: vec!["shop.test".into()],
            exclude: vec![],
        })
        .unwrap();
        assert!(app.agent_allowed("claude", "shop.test").is_ok());
        app.set_agent_mode(AgentMode::Advise).unwrap();
        assert!(
            app.agent_allowed("claude", "shop.test")
                .unwrap_err()
                .contains("advise mode")
        );
        assert!(
            app.agent_allowed("human", "anything.test").is_ok(),
            "the operator is never limited"
        );

        let target = Target {
            scheme: "https".into(),
            host: "shop.test".into(),
            port: 443,
        };
        let tab = app
            .prepare_request(
                "claude",
                &target,
                "GET /api/users/8 HTTP/1.1\nHost: shop.test\n\n",
                "read user 8",
            )
            .unwrap();
        assert_eq!(tab.name, "claude: read user 8");
        assert!(tab.sends.is_empty(), "prepared, not sent");
        assert!(
            app.prepare_request("claude", &target, "not a request", "")
                .is_err()
        );
        let last = app.timeline(1).unwrap().pop().unwrap();
        assert!(last.text.contains("Prepared"));
        assert_eq!(
            last.kind, "prepared",
            "a step within the plan, not a new plan"
        );

        // The choice survives a restart.
        let reopened = App::open(Config::default(), app.inner.data_dir.clone()).unwrap();
        assert_eq!(reopened.agent_mode(), AgentMode::Advise);
    }

    #[test]
    fn proxy_password_is_checked_only_when_set() {
        let app = test_app("password");
        assert!(app.proxy_password_ok(None), "no password configured: open");
        app.set_proxy_password(Some("s3cret".into()));
        assert!(app.proxy_password_ok(Some("s3cret")));
        assert!(!app.proxy_password_ok(Some("wrong")));
        assert!(!app.proxy_password_ok(None));
    }

    #[tokio::test]
    async fn subscribers_receive_events() {
        let app = test_app("events");
        let mut rx = app.subscribe();
        app.set_ai_paused(true);
        app.set_ai_paused(true); // no change, no second event
        app.set_ai_paused(false);
        assert!(matches!(
            rx.recv().await,
            Ok(Event::AgentMode {
                mode: AgentMode::Off
            })
        ));
        assert!(matches!(
            rx.recv().await,
            Ok(Event::AgentMode {
                mode: AgentMode::Act
            })
        ));
        assert!(!app.status().ai_paused);
    }

    #[test]
    fn agents_crlf_requests_become_editor_text() {
        let app = test_app("crlf-tab");
        let target = Target {
            scheme: "https".into(),
            host: "a.test".into(),
            port: 443,
        };
        let raw = "POST /login HTTP/1.1\r\nHost: a.test\r\n\r\nname=x";
        let tab = app
            .prepare_request("claude", &target, raw, "login")
            .unwrap();
        assert_eq!(tab.raw, "POST /login HTTP/1.1\nHost: a.test\n\nname=x");
        let tab = app
            .repeater_update(&tab.id, None, None, Some(raw.into()))
            .unwrap();
        assert!(!tab.raw.contains('\r'), "edits too");
    }

    #[tokio::test]
    async fn mcp_calls_are_kept_in_full_and_listed_as_previews() {
        let app = test_app("mcp-log");
        let mut rx = app.subscribe();
        let call = |tool: &str, response: String| McpCall {
            id: String::new(),
            ts: 0,
            agent: "claude".into(),
            method: "tools/call".into(),
            tool: Some(tool.into()),
            request: r#"{"id": "f1"}"#.into(),
            response,
            is_error: false,
            duration_ms: 3,
            request_size: 0,
            response_size: 0,
            color: None,
        };
        app.record_mcp_call(call("get_scope", "Scope: a.test".into()))
            .unwrap();
        let huge = "é".repeat(MCP_LOG_MAX); // 2 bytes each: cut on a char boundary
        app.record_mcp_call(call("get_flow", huge)).unwrap();

        let Ok(Event::McpCall { call: first }) = rx.recv().await else {
            panic!("an event per call")
        };
        assert_eq!(first.tool.as_deref(), Some("get_scope"));
        let listed = app.mcp_calls(10, None).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].response, "Scope: a.test", "oldest first");
        assert_eq!(listed[1].response.chars().count(), MCP_PREVIEW);
        assert!(listed[1].response_size > MCP_PREVIEW as u64);

        let full = app.mcp_call(&listed[1].id).unwrap().unwrap();
        assert!(
            full.response
                .ends_with(&format!("of {} bytes]", 2 * MCP_LOG_MAX))
        );
        assert_eq!(full.response.chars().count() as u64, full.response_size);
        assert!(
            app.mcp_calls(10, Some(&listed[1].id)).unwrap().len() == 1,
            "paging"
        );
        assert!(app.mcp_call("nope").unwrap().is_none());
        app.color_mcp_calls(&[listed[0].id.clone()], Some("purple"))
            .unwrap();
        assert_eq!(
            app.mcp_calls(10, None).unwrap()[0].color.as_deref(),
            Some("purple"),
            "a double-click in the MCP log sticks"
        );
        assert!(
            app.color_mcp_calls(&[listed[0].id.clone()], Some("pink"))
                .is_err()
        );
    }
}
