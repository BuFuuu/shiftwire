// Mirrors shiftwire-core's model.rs / events.rs.

export interface Status {
  version: string;
  pid: number;
  started_at: number;
  uptime_s: number;
  project: string;
  project_path: string;
  data_dir: string;
  api_addr: string;
  proxy_addr: string;
  proxy_running: boolean;
  proxy_error: string | null;
  /** Client connections open on the proxy port now. */
  proxy_connections: number;
  /** Unix ms of the latest request on the proxy port, 0 if none yet. */
  proxy_last_request: number;
  mcp_addr: string;
  mcp_running: boolean;
  mcp_error: string | null;
  ca_fingerprint: string;
  flows: number;
  dropped: number;
  agents: number;
  /** Agents active in the last 10 minutes, most recent first. */
  agent_list: { name: string; last_seen: number }[];
  ai_paused: boolean;
  agent_mode: AgentMode;
  /** The AI browser shiftWire opens (status bar → Open). */
  browser: { running: boolean; binary: string | null; proxy_port: number | null; error: string | null };
  /** What agents see of the target's names (Settings → Masking). */
  masking: Masking;
}

export type MaskLevel = 'off' | 'addresses' | 'content' | 'strict';

export interface Masking {
  level: MaskLevel;
  /** A real site and its stand-in (`target1.masked`); an empty alias gets the next free one. */
  sites: { real: string; alias: string }[];
  /** Strict: names replaced by `[word1]`, `[word2]`, ... */
  words: string[];
  /** Strict: hosts agents may see as they are (`*.gstatic.com`). */
  allow: string[];
  /** Strict: aliases given to other hosts so far (`h1.masked`). */
  auto: { real: string; alias: string }[];
}

export type AgentMode = 'act' | 'advise' | 'off';

/** A match & replace / alert rule (per project). */
export interface Rule {
  id: string;
  name: string;
  enabled: boolean;
  mode: 'replace' | 'alert';
  part: 'headers' | 'body';
  pattern: string;
  replace: string;
  when_missing: boolean;
  /** Alert mode: stop the request instead of sending it. */
  block: boolean;
}

export interface RuleAlert {
  rule: string;
  /** The rule stopped the request (it was not sent). */
  blocked: boolean;
  flow_id: string;
  seq: number;
  method: string;
  url: string;
}

/** A project is a folder holding project.json and flows.db. */
export interface Project {
  id: string;
  name: string;
  created_at: number;
  path: string;
}

export interface ProjectInfo extends Project {
  loaded: boolean;
  flows: number | null;
  size: number;
}

export interface FlowSummary {
  id: string;
  /** The # in History: stable and unique within the project. */
  seq: number;
  ts: number;
  actor: string;
  method: string;
  scheme: string;
  host: string;
  port: number;
  /** Path including the query string. */
  path: string;
  status: number | null;
  mime: string | null;
  size: number;
  duration_ms: number;
  error: string | null;
  tags: string[];
  color: string | null;
  note: string | null;
  /** Latest "important" mark: severity and reason. */
  mark: string | null;
  mark_reason: string | null;
  /** The agent's log_intent id it was sent under. */
  intent: string | null;
  /** Which way it came in: proxy, browser, tunnel, repeater, mcp (filter `via:`); `invisible` on old rows. */
  via: string;
}

/** One MCP request from an agent and exactly what it got back (the MCP log). */
export interface McpCall {
  id: string;
  ts: number;
  agent: string;
  /** JSON-RPC method: tools/call, initialize, tools/list, ... */
  method: string;
  tool: string | null;
  /** The tool's arguments (or the method's params) as JSON; in lists only the start. */
  request: string;
  /** What the agent got back; in lists only the start. */
  response: string;
  is_error: boolean;
  duration_ms: number;
  request_size: number;
  response_size: number;
  /** The operator's highlight, like a flow's (double-click toggles purple). */
  color?: string | null;
}

export const SEVERITIES = ['info', 'low', 'medium', 'high', 'critical'] as const;

export interface TimelineEntry {
  id: string;
  ts: number;
  actor: string;
  kind: 'intent' | 'mark' | 'operator' | 'prepared';
  text: string;
  flow_id: string | null;
  severity: string | null;
}

export interface Target {
  scheme: 'http' | 'https';
  host: string;
  port: number;
}

export interface RepeaterTab {
  id: string;
  name: string;
  target: Target;
  raw: string;
  sends: string[];
}

export const COLORS = ['red', 'orange', 'yellow', 'green', 'blue', 'purple', 'gray'] as const;

export type WireEvent =
  | { type: 'hello'; status: Status }
  | { type: 'status'; status: Status }
  | { type: 'mcp_call'; call: McpCall }
  | { type: 'mcp_calls_colored'; ids: string[]; color: string | null }
  | { type: 'flow_added'; flow: FlowSummary; matches?: boolean }
  | { type: 'timeline'; entry: TimelineEntry }
  | { type: 'flow_updated'; flow: FlowSummary; matches: boolean }
  | { type: 'subscribed'; filter: string }
  | { type: 'filter_error'; message: string }
  | { type: 'agent_mode'; mode: AgentMode }
  | { type: 'repeater_changed'; tabs: RepeaterTab[] }
  | { type: 'scope_changed'; scope: { include: string[]; exclude: string[] } }
  | { type: 'project_changed'; project: Project }
  | ({ type: 'rule_alert' } & RuleAlert)
  | { type: 'resync'; missed: number };

export interface BodyView {
  size: number;
  truncated: boolean;
  decoded: string | null;
  text: string | null;
  base64: string | null;
}

export interface Message {
  head: string;
  body: BodyView;
}

export interface FlowDetail extends FlowSummary {
  source: string;
  http_version: string;
  request: Message;
  response: Message | null;
}

/** The run of requests one request belongs to (see shiftwire-core `Burst`). */
export interface Burst {
  actor: string;
  via: string;
  count: number;
  first_seq: number;
  first_ts: number;
  last_seq: number;
  last_ts: number;
  gap_ms: number;
  largest_gap_ms: number;
  gap_before_ms: number | null;
  gap_after_ms: number | null;
}
