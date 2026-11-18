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
  ca_fingerprint: string;
  flows: number;
  dropped: number;
  agents: number;
  ai_paused: boolean;
  agent_mode: AgentMode;
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
}

export interface RuleAlert {
  rule: string;
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
