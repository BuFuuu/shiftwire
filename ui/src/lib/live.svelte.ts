// Live state pushed by the daemon over one WebSocket. Reconnects forever, so
// the UI survives daemon restarts and can be opened or closed at any time.
//
// Filtering happens on the server (one parser, see shiftwire-core/src/filter.rs):
// history is queried with the filter, and the socket only pushes matching flows.

import { invalidate } from './detail';
import { loadTabs, repeater } from './repeater.svelte';
import { nav } from './nav.svelte';
import type { AgentMode, FlowSummary, WireEvent, Status, TimelineEntry, RuleAlert, McpCall } from './types';

const MAX_FLOWS = 100_000;
/** Rows per request: small pages keep every filter change instant. */
const PAGE = 2000;
const FILTER_KEY = 'shiftwire.filter';
const PINS_KEY = 'shiftwire.pins';

/** A lasting filter chip: hide rows containing `text`, or show only rows containing it. */
export interface Pin {
  text: string;
  /** `after`: hide the requests `#from-to` in `text` (History's Clear button). */
  mode: 'hide' | 'show' | 'after';
  /** Shown instead of `text`, e.g. the time of that flow. */
  label?: string;
  /** Made by "Show in History" elsewhere (Site Map, alerts): the next one replaces it. */
  jump?: boolean;
  on: boolean;
}

export const live = $state({
  connected: false,
  status: null as Status | null,
  agentMode: 'act' as AgentMode,
  filter: localStorage.getItem(FILTER_KEY) ?? '',
  pins: JSON.parse(localStorage.getItem(PINS_KEY) ?? '[]') as Pin[],
  filterError: null as string | null,
  /** The project's scope (Settings), kept current by the server's events. */
  scope: null as { include: string[]; exclude: string[] } | null,
  /** History and Site Map show only in-scope hosts: on by default, once a scope is set. */
  scopeOnly: localStorage.getItem('shiftwire.scopeOnly') !== 'false',
  /** History shows Anthropic / Claude traffic (agents' own model API calls, tunneled untouched). */
  showClaude: localStorage.getItem('shiftwire.showClaude') !== 'false',
});

export function setShowClaude(on: boolean): void {
  live.showClaude = on;
  localStorage.setItem('shiftwire.showClaude', String(on));
  subscribe();
  void loadHistory();
}

/** Hosts hidden when `showClaude` is off: each domain and all its subdomains. */
const CLAUDE_HOSTS = ['anthropic.com', 'claude.ai', 'claude.com'];
const claudeFilter = () =>
  live.showClaude ? '' : `-(${CLAUDE_HOSTS.map((d) => `host:=${d} OR host:=*.${d}`).join(' OR ')})`;

export const hasScope = () => (live.scope?.include.length ?? 0) > 0;

export function setScopeOnly(on: boolean): void {
  live.scopeOnly = on;
  localStorage.setItem('shiftwire.scopeOnly', String(on));
  subscribe();
  void loadHistory();
}

/** The scope as a filter: `*.shop.test` is the domain and every subdomain; exclusions win. */
function scopeFilter(): string {
  if (!live.scopeOnly || !live.scope || !hasScope()) return '';
  const host = (p: string) => (p.startsWith('*.') ? `host:=${p.slice(2)} OR host:=${p}` : `host:=${p}`);
  const include = `(${live.scope.include.map(host).join(' OR ')})`;
  const exclude = live.scope.exclude.map((p) => `-(${host(p)})`);
  return [include, ...exclude].join(' ');
}

class History {
  /** The newest matching flows, in time order. `raw`: rows are replaced, never mutated. */
  flows = $state.raw<FlowSummary[]>([]);
  /** How many flows match the filter in total (older ones load on scroll). */
  total = $state(0);
  loadingOlder = $state(false);
}
export const history = new History();

const AGENT_FLOWS = 2000;

/** The AI Timeline: agents' intents, marks, operator messages, and agents' requests. */
class Timeline {
  entries = $state.raw<TimelineEntry[]>([]);
  flows = $state.raw<FlowSummary[]>([]);
  /** Marks by agents not yet looked at (shown as a badge). */
  unseenMarks = $state(0);
  /** The newest agent mark, shown briefly as a toast. */
  toast = $state<TimelineEntry | null>(null);
}
export const timeline = new Timeline();

/** The MCP log: every MCP request agents made, oldest first (previews; the full call is fetched on demand). */
export const mcpLog = $state({ calls: [] as McpCall[] });
const MCP_CALLS = 5000;

async function loadMcpLog(): Promise<void> {
  mcpLog.calls = await fetch(`/api/mcp-calls?limit=${MCP_CALLS}`).then((r) => r.json());
}

/** The latest alert from a match & replace / alert rule, shown briefly as a toast. */
export const alerts = $state({ last: null as RuleAlert | null });
let alertTimer: ReturnType<typeof setTimeout> | undefined;

export const isAgent = (actor: string) => actor !== 'proxy' && actor !== 'human';

async function loadTimeline(): Promise<void> {
  const [entries, flows] = await Promise.all([
    fetch('/api/timeline?limit=2000').then((r) => r.json()),
    fetch(`/api/flows?limit=${AGENT_FLOWS}&filter=is:ai`).then((r) => r.json()),
  ]);
  timeline.entries = entries;
  timeline.flows = flows;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;
function onTimeline(entry: TimelineEntry): void {
  timeline.entries = [...timeline.entries, entry];
  if (entry.kind === 'mark' && isAgent(entry.actor)) {
    timeline.unseenMarks += 1;
    timeline.toast = entry;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (timeline.toast = null), 7000);
  }
}

let socket: WebSocket | null = null;
let attempt = 0;
let loading = false;
let pending: FlowSummary[] = [];
let seen = new Set<string>();
let flushScheduled = false;
let inflight: AbortController | null = null;

export function connect(): void {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  socket = ws;
  ws.onopen = () => {
    attempt = 0;
    live.connected = true;
  };
  ws.onmessage = (msg) => handle(JSON.parse(msg.data) as WireEvent);
  ws.onclose = () => {
    live.connected = false;
    socket = null;
    setTimeout(connect, Math.min(5000, 250 * 2 ** attempt++));
  };
}

/** Apply a filter: the socket follows it and history is reloaded with it. */
export function setFilter(text: string): void {
  live.filter = text.trim();
  localStorage.setItem(FILTER_KEY, live.filter);
  subscribe();
  void loadHistory();
}

/** "Show in History" from another view: a removable chip, not an edit of the main filter. */
export function jumpTo(text: string, label?: string): void {
  setPins([...live.pins.filter((p) => !p.jump), { text, mode: 'show', on: true, jump: true, label }]);
  nav.view = 'history';
}

/** Adds a lasting filter chip (once). */
export function addPin(mode: Pin['mode'], text: string): void {
  const value = text.trim().replaceAll('"', '');
  if (value && !live.pins.some((p) => p.text === value && p.mode === mode)) {
    setPins([...live.pins, { text: value, mode, on: true }]);
  }
}

export function setPins(pins: Pin[]): void {
  live.pins = pins;
  localStorage.setItem(PINS_KEY, JSON.stringify(pins));
  subscribe();
  void loadHistory();
}

/** The main filter plus the lasting ones, as one expression for the server. */
/** `1-50`, `#1–50` or just `50` (= 1-50) -> [from, to]; null if it is not a range. */
export function idRange(text: string): [number, number] | null {
  const m = /^#?(\d+)(?:\s*[-–]\s*#?(\d+))?$/.exec(text.trim());
  if (!m) return null;
  const [from, to] = m[2] ? [Number(m[1]), Number(m[2])] : [1, Number(m[1])];
  return from <= to ? [from, to] : [to, from];
}

export function effectiveFilter(): string {
  // A chip is row text (`*mozilla*`), or one field term as in the filter (`status:3*`).
  const field = /^(host|path|url|method|actor|via|color|mime|type|note|tag|status|size|ms|time|id|#|is|has):\S+$/i;
  const terms = (t: string) => t.trim().split(/\s+/);
  const quote = (t: string) =>
    terms(t).every((w) => w === 'OR' || field.test(w.replace(/^\(+|\)+$/g, ''))) ? `(${t})` : `"${t.replaceAll('"', '')}"`;
  const on = live.pins.filter((p) => p.on);
  const show = on.filter((p) => p.mode === 'show').map((p) => quote(p.text));
  const parts = [
    live.filter ? `(${live.filter})` : '',
    scopeFilter(),
    claudeFilter(),
    show.length ? `(${show.join(' OR ')})` : '',
    ...on.filter((p) => p.mode === 'hide').map((p) => `-${quote(p.text)}`),
    ...on.flatMap((p) => {
      const r = p.mode === 'after' ? idRange(p.text) : null;
      return r ? [`NOT (id:>=${r[0]} id:<=${r[1]})`] : [];
    }),
  ];
  return parts.filter(Boolean).join(' ');
}

function subscribe(): void {
  if (socket?.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({ type: 'subscribe', filter: effectiveFilter() }));
  }
}

function handle(event: WireEvent): void {
  switch (event.type) {
    case 'hello':
      // Reconnected to a restarted daemon (maybe a new build): load its UI, not the old one.
      if (live.status && live.status.started_at !== event.status.started_at) {
        location.reload();
        return;
      }
      live.status = event.status;
      live.agentMode = event.status.agent_mode;
      // The scope first: with "In scope" on, History must not flash everything else.
      void fetch('/api/scope')
        .then((r) => r.json())
        .then((scope) => (live.scope = scope))
        .finally(() => {
          subscribe();
          void loadHistory();
        });
      void loadTabs();
      void loadTimeline();
      void loadMcpLog();
      break;
    case 'mcp_calls_colored':
      mcpLog.calls = mcpLog.calls.map((c) => (event.ids.includes(c.id) ? { ...c, color: event.color } : c));
      break;
    case 'mcp_call':
      mcpLog.calls = [...mcpLog.calls, event.call].slice(-MCP_CALLS);
      break;
    case 'rule_alert':
      alerts.last = event;
      clearTimeout(alertTimer);
      alertTimer = setTimeout(() => (alerts.last = null), 7000);
      break;
    case 'project_changed':
      // Another project's data: start over (filter chips and prefs stay, they live in the browser).
      location.reload();
      break;
    case 'scope_changed':
      live.scope = event.scope;
      if (live.scopeOnly) {
        subscribe();
        void loadHistory();
      }
      break;
    case 'status':
      live.status = event.status;
      live.agentMode = event.status.agent_mode;
      break;
    case 'repeater_changed':
      repeater.tabs = event.tabs;
      break;
    case 'agent_mode':
      live.agentMode = event.mode;
      break;
    case 'flow_added':
      if (live.status) live.status.flows += 1;
      if (isAgent(event.flow.actor)) timeline.flows = [...timeline.flows, event.flow].slice(-AGENT_FLOWS);
      if (event.matches !== false) {
        pending.push(event.flow);
        scheduleFlush();
      }
      break;
    case 'timeline':
      onTimeline(event.entry);
      break;
    case 'flow_updated':
      invalidate(event.flow.id);
      if (timeline.flows.some((f) => f.id === event.flow.id)) {
        timeline.flows = timeline.flows.map((f) => (f.id === event.flow.id ? event.flow : f));
      }
      if (!event.matches) {
        removeFlows([event.flow.id]);
      } else if (seen.has(event.flow.id)) {
        replaceFlows([event.flow]);
      } else {
        pending.push(event.flow); // newly matches the filter (e.g. just tagged): add it
        scheduleFlush();
      }
      break;
    case 'filter_error':
      live.filterError = event.message;
      break;
    case 'subscribed':
      live.filterError = null;
      break;
    case 'resync':
      void loadHistory();
      break;
  }
}

async function fetchPage(before: string | null, signal?: AbortSignal) {
  const query = new URLSearchParams({ limit: String(PAGE), filter: effectiveFilter() });
  if (before) query.set('before', before);
  return fetch(`/api/flows?${query}`, { signal });
}

/** (Re)load the newest page; live events that arrive meanwhile are merged after.
 *  A newer call cancels an older one still in flight (fast typing in the filter). */
async function loadHistory(): Promise<void> {
  inflight?.abort();
  const controller = new AbortController();
  inflight = controller;
  loading = true;
  pending = [];
  try {
    const res = await fetchPage(null, controller.signal);
    if (res.ok) {
      const flows = (await res.json()) as FlowSummary[];
      if (controller.signal.aborted) return;
      history.flows = flows;
      history.total = Number(res.headers.get('x-total-count') ?? flows.length);
      seen = new Set(flows.map((f) => f.id));
      live.filterError = null;
    } else if (res.status === 400) {
      live.filterError = await res.text();
    }
  } catch (e) {
    if ((e as Error).name !== 'AbortError') throw e;
  } finally {
    if (inflight === controller) {
      inflight = null;
      loading = false;
      scheduleFlush();
    }
  }
}

/** Load the page before the oldest loaded row. Returns how many rows were added. */
export async function loadOlder(): Promise<number> {
  const oldest = history.flows[0];
  if (!oldest || loading || history.loadingOlder || history.flows.length >= history.total) return 0;
  history.loadingOlder = true;
  try {
    const res = await fetchPage(oldest.id);
    if (!res.ok || history.flows[0] !== oldest) return 0; // the filter changed meanwhile
    const older = ((await res.json()) as FlowSummary[]).filter((f) => !seen.has(f.id));
    older.forEach((f) => seen.add(f.id));
    history.flows = older.concat(history.flows).slice(-MAX_FLOWS);
    return older.length;
  } finally {
    history.loadingOlder = false;
  }
}

/** Swap in newer versions of rows already in the list (tags, color, note). */
export function replaceFlows(updated: FlowSummary[]): void {
  const byId = new Map(updated.map((f) => [f.id, f]));
  if (!history.flows.some((f) => byId.has(f.id))) return;
  history.flows = history.flows.map((f) => byId.get(f.id) ?? f);
}

function removeFlows(ids: string[]): void {
  const gone = new Set(ids);
  const before = history.flows.length;
  history.flows = history.flows.filter((f) => !gone.has(f.id));
  history.total = Math.max(0, history.total - (before - history.flows.length));
  gone.forEach((id) => seen.delete(id));
}

/** Batch live rows into one update per frame, so bursts of traffic stay smooth.
 *  Browsers pause frames for minimized or covered windows, so a timer backs it
 *  up: the list stays current (and `pending` bounded) even when nobody looks. */
function scheduleFlush(): void {
  if (flushScheduled || loading) return;
  flushScheduled = true;
  const flush = () => {
    if (!flushScheduled) return; // the other trigger already ran
    flushScheduled = false;
    // Requests overlap and finish in any order, so dedupe by id, not by position.
    const fresh = pending.filter((f) => !seen.has(f.id));
    fresh.forEach((f) => seen.add(f.id));
    history.total += fresh.length;
    pending = [];
    if (fresh.length) history.flows = insertByTime(history.flows, fresh).slice(-MAX_FLOWS);
  };
  requestAnimationFrame(flush);
  setTimeout(flush, 100);
}

/** Flows sort by start time (ULID ties break by id), like the server's history. */
export function before(a: FlowSummary, b: FlowSummary): boolean {
  return a.ts < b.ts || (a.ts === b.ts && a.id < b.id);
}

/** Position of `f` in a time-ordered list (binary search). */
export function timeIndex(list: FlowSummary[], f: FlowSummary): number {
  let lo = 0;
  let hi = list.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (before(list[mid], f)) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Requests finish out of order; keep the list in start order. Appends stay O(1). */
function insertByTime(list: FlowSummary[], fresh: FlowSummary[]): FlowSummary[] {
  const out = list.slice();
  for (const f of fresh.sort((a, b) => (before(a, b) ? -1 : 1))) {
    const last = out.at(-1);
    if (!last || before(last, f)) out.push(f);
    else out.splice(timeIndex(out, f), 0, f);
  }
  return out;
}
