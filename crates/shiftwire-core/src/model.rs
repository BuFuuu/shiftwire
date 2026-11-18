//! Plain data types shared by all adapters.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub pid: u32,
    pub started_at: u64,
    pub uptime_s: u64,
    /// Name and folder of the loaded project.
    pub project: String,
    pub project_path: String,
    pub data_dir: String,
    pub api_addr: String,
    pub proxy_addr: String,
    pub proxy_running: bool,
    /// Why the proxy is not running (e.g. port in use), if it isn't.
    pub proxy_error: Option<String>,
    /// SHA-256 of the shiftWire CA certificate, to compare with what clients trust.
    pub ca_fingerprint: String,
    pub flows: u64,
    /// Captures skipped because the store could not keep up (forwarding still worked).
    pub dropped: u64,
    pub agents: u32,
    /// True when `agent_mode` is `off`.
    pub ai_paused: bool,
    pub agent_mode: AgentMode,
}

/// What AI agents may do. The operator chooses; agents cannot change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Agents send requests themselves (within the scope).
    #[default]
    Act,
    /// Agents read and analyze, and prepare requests in the Repeater for the operator to send.
    Advise,
    /// Every agent request is refused.
    Off,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentMode::Act => "act",
            AgentMode::Advise => "advise",
            AgentMode::Off => "off",
        }
    }
}

/// One captured exchange (or tunnel). Bodies are stored up to `max_body_bytes`.
#[derive(Debug, Clone, Default)]
pub struct Flow {
    pub id: String,
    /// The request's number in its project (#1, #2, ...), assigned when stored.
    pub seq: u64,
    /// Unix milliseconds.
    pub ts_start: u64,
    pub ts_end: u64,
    /// Who sent it: an agent name from proxy credentials, "proxy" otherwise.
    pub actor: String,
    /// "proxy" | "tunnel" (later: "repeater", "mcp").
    pub source: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub http_version: String,
    pub status: Option<u16>,
    pub mime: Option<String>,
    pub req_head: String,
    pub req_body: Vec<u8>,
    pub req_size: u64,
    pub req_trunc: bool,
    pub resp_head: String,
    pub resp_body: Vec<u8>,
    pub resp_size: u64,
    pub resp_trunc: bool,
    pub error: Option<String>,
    pub tags: Vec<String>,
    pub color: Option<String>,
    pub note: Option<String>,
    /// Severity of the latest "important" mark by an agent or the operator.
    pub mark: Option<String>,
    pub mark_reason: Option<String>,
    /// The agent's intent (log_intent id) this request was sent under.
    pub intent: Option<String>,
}

impl Flow {
    pub fn duration_ms(&self) -> u64 {
        self.ts_end.saturating_sub(self.ts_start)
    }

    pub fn summary(&self) -> FlowSummary {
        let path = match &self.query {
            Some(q) => format!("{}?{q}", self.path),
            None => self.path.clone(),
        };
        FlowSummary {
            id: self.id.clone(),
            seq: self.seq,
            ts: self.ts_start,
            actor: self.actor.clone(),
            method: self.method.clone(),
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            port: self.port,
            path,
            status: self.status,
            mime: self.mime.clone(),
            size: self.resp_size,
            duration_ms: self.duration_ms(),
            error: self.error.clone(),
            tags: self.tags.clone(),
            color: self.color.clone(),
            note: self.note.clone(),
            mark: self.mark.clone(),
            mark_reason: self.mark_reason.clone(),
            intent: self.intent.clone(),
        }
    }
}

/// One row in the live history list. Kept small: the UI may hold 100k of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowSummary {
    pub id: String,
    /// The `#` shown in History: stable, unique within the project.
    pub seq: u64,
    pub ts: u64,
    pub actor: String,
    pub method: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Path including the query string.
    pub path: String,
    pub status: Option<u16>,
    pub mime: Option<String>,
    pub size: u64,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub tags: Vec<String>,
    /// One of [`COLORS`].
    pub color: Option<String>,
    pub note: Option<String>,
    /// Severity of the latest "important" mark (see [`SEVERITIES`]).
    pub mark: Option<String>,
    pub mark_reason: Option<String>,
    /// The agent's intent (log_intent id) it was sent under: filter `intent:<id>`.
    pub intent: Option<String>,
}

/// A Repeater tab: a request being edited and the flows sent from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepeaterTab {
    pub id: String,
    pub name: String,
    pub target: crate::sender::Target,
    /// The request as written in the editor.
    pub raw: String,
    /// Flow ids sent from this tab, oldest first.
    pub sends: Vec<String>,
}

/// How important a mark is, least to most.
pub const SEVERITIES: [&str; 5] = ["info", "low", "medium", "high", "critical"];

/// One entry on the AI Timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub id: String,
    pub ts: u64,
    /// The agent, or "human" for the operator.
    pub actor: String,
    /// "intent" (what an agent is about to do), "mark" (a request flagged as
    /// important), "prepared" (a request an agent put in the Repeater for the
    /// operator) or "operator" (a message from the human to the agents).
    pub kind: String,
    pub text: String,
    pub flow_id: Option<String>,
    pub severity: Option<String>,
}

/// Row highlight colors, in palette order (keys 1-7 in the UI).
pub const COLORS: [&str; 7] = ["red", "orange", "yellow", "green", "blue", "purple", "gray"];

/// Everything about one flow, ready for display: bodies are decoded
/// (`Content-Encoding`) and given as text when they are UTF-8.
#[derive(Debug, Clone, Serialize)]
pub struct FlowDetail {
    #[serde(flatten)]
    pub summary: FlowSummary,
    pub source: String,
    pub http_version: String,
    pub request: Message,
    /// `None` when no response arrived (connection error, tunnel, ...).
    pub response: Option<Message>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    /// First line plus headers.
    pub head: String,
    pub body: BodyView,
}

#[derive(Debug, Clone, Serialize)]
pub struct BodyView {
    /// Size on the wire.
    pub size: u64,
    /// Stored bytes were cut at `max_body_bytes`.
    pub truncated: bool,
    /// The `Content-Encoding` that was undone for display, if any.
    pub decoded: Option<String>,
    /// The body as text, when it is valid UTF-8 ...
    pub text: Option<String>,
    /// ... otherwise base64.
    pub base64: Option<String>,
}

impl From<Flow> for FlowDetail {
    fn from(f: Flow) -> Self {
        let request = Message::new(f.req_head.clone(), &f.req_body, f.req_size, f.req_trunc);
        let response = (!f.resp_head.is_empty())
            .then(|| Message::new(f.resp_head.clone(), &f.resp_body, f.resp_size, f.resp_trunc));
        Self {
            summary: f.summary(),
            source: f.source,
            http_version: f.http_version,
            request,
            response,
        }
    }
}

impl Message {
    fn new(head: String, body: &[u8], size: u64, truncated: bool) -> Self {
        let encoding = header(&head, "content-encoding");
        let decoded = encoding.and_then(|e| crate::decode::decode(body, e));
        let bytes = decoded.as_deref().unwrap_or(body);
        let (text, base64) = match std::str::from_utf8(bytes) {
            Ok(text) => (Some(text.to_string()), None),
            Err(_) => {
                use base64::Engine;
                (
                    None,
                    Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                )
            }
        };
        let body = BodyView {
            size,
            truncated,
            decoded: decoded
                .is_some()
                .then(|| encoding.unwrap_or_default().to_string()),
            text,
            base64,
        };
        Self { head, body }
    }
}

/// A header value from a stored head (first line skipped, name case-insensitive).
pub fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(n, _)| n.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_decodes_and_picks_text_or_base64() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, b"{\"ok\":true}").unwrap();
        let flow = Flow {
            req_head: "POST /x HTTP/1.1\r\nHost: a\r\n".into(),
            req_body: vec![0xff, 0x00],
            req_size: 2,
            resp_head: "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n".into(),
            resp_body: gz.finish().unwrap(),
            ..Flow::default()
        };
        let d = FlowDetail::from(flow);
        assert_eq!(d.request.body.base64.as_deref(), Some("/wA="));
        let resp = d.response.unwrap();
        assert_eq!(resp.body.text.as_deref(), Some("{\"ok\":true}"));
        assert_eq!(resp.body.decoded.as_deref(), Some("gzip"));
    }

    #[test]
    fn header_lookup_ignores_case_and_first_line() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n";
        assert_eq!(
            header(head, "content-type"),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(header(head, "HTTP/1.1 200 OK"), None);
    }
}
