//! The event bus: one broadcast channel every adapter (UI, MCP, CLI) can listen to.

use serde::Serialize;
use tokio::sync::broadcast;

use crate::model::{AgentMode, FlowSummary, McpCall, RepeaterTab, TimelineEntry};
use crate::project::Project;
use crate::scope::Scope;

/// Serialized as `{"type": "...", ...}` so the UI can switch on `type`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    FlowAdded {
        flow: Box<FlowSummary>,
    },
    /// Tags, color or note changed.
    FlowUpdated {
        flow: Box<FlowSummary>,
    },
    /// The operator switched what agents may do.
    AgentMode {
        mode: AgentMode,
    },
    /// The Repeater tabs changed (any window, or an agent): the full list.
    RepeaterChanged {
        tabs: Vec<RepeaterTab>,
    },
    /// The operator changed which hosts agents may touch.
    ScopeChanged {
        scope: Scope,
    },
    /// A new AI Timeline entry: an agent's intent, a mark, or an operator message.
    Timeline {
        entry: TimelineEntry,
    },
    /// An alert rule matched (or found something missing) in a proxied request.
    RuleAlert {
        rule: String,
        /// The rule stopped the request (it was not sent).
        blocked: bool,
        flow_id: String,
        seq: u64,
        method: String,
        url: String,
    },
    /// An agent made an MCP request (request and response cut to a preview).
    McpCall {
        call: McpCall,
    },
    /// The operator colored MCP calls (or cleared the color).
    McpCallsColored {
        ids: Vec<String>,
        color: Option<String>,
    },
    /// The operator switched to another project: everything shown is now stale.
    ProjectChanged {
        project: Project,
    },
}

#[derive(Clone)]
pub(crate) struct Bus(broadcast::Sender<Event>);

impl Bus {
    pub fn new() -> Self {
        // Slow subscribers lag (and resync) instead of slowing the proxy down.
        Self(broadcast::channel(4096).0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
    }

    pub fn emit(&self, event: Event) {
        let _ = self.0.send(event); // no subscribers is fine
    }
}
