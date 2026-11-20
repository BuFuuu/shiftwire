// Live state pushed by the daemon over one WebSocket. Reconnects forever, so
// the UI survives daemon restarts and can be opened or closed at any time.
//
// Filtering happens on the server (one parser, see shiftwire-core/src/filter.rs):
// history is queried with the filter, and the socket only pushes matching flows.

import { invalidate } from './detail';
import { loadTabs, repeater } from './repeater.svelte';
import { nav } from './nav.svelte';
import type { AgentMode, FlowSummary, WireEvent, Status, TimelineEntry, RuleAlert, McpCall } from './types';

/** Rows per request: small pages keep every filter change and jump instant. */
const PAGE = 500;
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

export type SortKey = 'n' | 'actor' | 'via' | 'method' | 'host' | 'path' | 'status' | 'size' | 'duration_ms';

/** History holds a slot for every match, so the scrollbar is the whole list; the
 *  rows in view are loaded by position (`ensureRows`). The server sorts. */
class History {
  /** Every match in the order shown; `undefined`: not loaded yet. `raw`: replaced, never mutated. */
  rows = $state.raw<(FlowSummary | undefined)[]>([]);
  /** How many flows match the filter. */
  total = $derived(this.rows.length);
  /** `n` ascending is time order: the only order live rows are inserted into directly. */
  sort = $state<{ key: SortKey; desc: boolean }>({ key: 'n', desc: false });
  /** Kept current by the view, so a reload shows the same place: the first row in view,
   *  and whether it sticks to the bottom (the newest rows). */
  viewStart = 0;
  follow = true;
  /** The selected row (kept by the view): a reload for a new filter or sort centers on it. */
  selected: string | null = null;
  /** Set by such a reload: the row the view should scroll to. */
  reveal = $state<{ index: number } | null>(null);
  /** The burst gap for the line next to the numbers (the last one picked in the menu). */
  burstGap = $state(Number(localStorage.getItem('shiftwire.burstGap') ?? 2000));
}
export const history = new History();
const timeOrder = () => history.sort.key === 'n' && !history.sort.desc;

/** The burst gap (History's lines, Show / Hide burst), kept in the browser. */
export function setBurstGap(ms: number): void {
  history.burstGap = ms;
  localStorage.setItem('shiftwire.burstGap', String(ms));
}

export function setSort(key: SortKey): void {
  history.sort = history.sort.key === key ? { key, desc: !history.sort.desc } : { key, desc: false };
  void loadHistory();
}

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
  const field = /^(host|path|url|method|actor|via|color|mime|type|note|tag|status|size|ms|time|id|ts|#|is|has):\S+$/i;
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
      void loadHistory(false);
      break;
  }
}

/** Up to `limit` matches from position `offset` (none: the last page). */
function fetchWindow(offset: number | null, limit: number, signal?: AbortSignal, around?: string | null) {
  const query = new URLSearchParams({
    limit: String(limit),
    filter: effectiveFilter(),
    sort: history.sort.key,
    desc: String(history.sort.desc),
  });
  if (offset !== null) query.set('offset', String(offset));
  if (around) query.set('around', around);
  return fetch(`/api/flows?${query}`, { signal });
}

/** Bumped by every reload: pages still in flight from before are dropped. */
let generation = 0;
let pages = new Map<number, Promise<void>>();

/** Puts fetched rows at their positions (rows the client does not know of yet arrive as live events). */
function place(flows: FlowSummary[], offset: number) {
  const out = history.rows.slice();
  flows.forEach((f, j) => {
    const i = offset + j;
    if (i >= out.length) return;
    const old = out[i];
    if (old && old.id !== f.id) seen.delete(old.id);
    out[i] = f;
    seen.add(f.id);
  });
  history.rows = out;
}

/** Load the pages that have unloaded rows in `start..end`. */
export function ensureRows(start: number, end: number): Promise<void> {
  if (loading) return Promise.resolve(); // positions are about to change; the new rows trigger this again
  const rows = history.rows;
  const waits: Promise<void>[] = [];
  for (let i = Math.max(0, start); i < Math.min(end, rows.length); i++) {
    if (rows[i]) continue;
    const page = Math.floor(i / PAGE);
    waits.push(pages.get(page) ?? loadPage(page));
    i = (page + 1) * PAGE - 1; // on to the next page
  }
  return Promise.all(waits).then(() => {});
}

function loadPage(page: number): Promise<void> {
  const g = generation;
  const done = (async () => {
    try {
      const res = await fetchWindow(page * PAGE, PAGE);
      if (!res.ok || g !== generation) return;
      const flows = (await res.json()) as FlowSummary[];
      if (g === generation) place(flows, Number(res.headers.get('x-offset')));
    } finally {
      if (g === generation) pages.delete(page);
    }
  })();
  pages.set(page, done);
  return done;
}

/** The loaded rows in `start..end`, after loading what is missing. */
export async function rowsIn(start: number, end: number): Promise<FlowSummary[]> {
  await ensureRows(start, end);
  return history.rows.slice(start, end).filter((f) => f !== undefined);
}

/** The newest matching flow (History's Clear button). */
export async function newestMatch(): Promise<FlowSummary | undefined> {
  const res = await fetch(`/api/flows?${new URLSearchParams({ limit: '1', filter: effectiveFilter() })}`);
  return res.ok ? ((await res.json()) as FlowSummary[])[0] : undefined;
}

/** (Re)load after the filter or sort changed: around the selected row if it still
 *  matches (`keepSelected`), else at the place in view (the bottom when following).
 *  Live events that arrive meanwhile are merged after. A newer call cancels an older
 *  one still in flight (fast typing in the filter). */
async function loadHistory(keepSelected = true): Promise<void> {
  inflight?.abort();
  const controller = new AbortController();
  inflight = controller;
  const g = ++generation;
  pages = new Map();
  loading = true;
  pending = [];
  try {
    const offset = history.follow ? null : Math.max(0, history.viewStart - 50);
    const res = await fetchWindow(offset, PAGE, controller.signal, keepSelected ? history.selected : null);
    if (res.ok) {
      const flows = (await res.json()) as FlowSummary[];
      if (controller.signal.aborted || g !== generation) return;
      seen = new Set();
      history.rows = new Array(Number(res.headers.get('x-total-count') ?? flows.length));
      place(flows, Number(res.headers.get('x-offset') ?? 0));
      const position = res.headers.get('x-position');
      if (position) history.reveal = { index: Number(position) };
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

/** In other orders a new or changed row can land anywhere: reload the place in view, at most a few times a second. */
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
function refreshSoon(): void {
  refreshTimer ??= setTimeout(() => {
    refreshTimer = undefined;
    void loadHistory(false); // a background refresh: do not pull the view back
  }, 300);
}

/** Swap in newer versions of rows already in the list (tags, color, note). */
export function replaceFlows(updated: FlowSummary[]): void {
  const byId = new Map(updated.map((f) => [f.id, f]));
  if (!updated.some((f) => seen.has(f.id))) return;
  history.rows = history.rows.map((f) => (f && byId.get(f.id)) || f); // map keeps unloaded slots
}

function removeFlows(ids: string[]): void {
  const gone = new Set(ids);
  if (!ids.some((id) => seen.has(id))) {
    if (!timeOrder()) refreshSoon(); // maybe one not loaded: the count changed
    return;
  }
  const out = history.rows.slice();
  for (let i = out.length - 1; i >= 0; i--) {
    const f = out[i];
    if (f && gone.has(f.id)) out.splice(i, 1);
  }
  history.rows = out;
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
    pending = [];
    if (!fresh.length) return;
    if (!timeOrder()) return refreshSoon();
    fresh.forEach((f) => seen.add(f.id));
    history.rows = insertByTime(history.rows, fresh);
  };
  requestAnimationFrame(flush);
  setTimeout(flush, 100);
}

/** Flows sort by start time (ULID ties break by id), like the server's history. */
export function before(a: FlowSummary, b: FlowSummary): boolean {
  return a.ts < b.ts || (a.ts === b.ts && a.id < b.id);
}

/** Requests finish out of order; keep the list in start order. New rows belong at
 *  or near the end, so walk back from there (an unloaded slot ends the walk). */
function insertByTime(list: (FlowSummary | undefined)[], fresh: FlowSummary[]): (FlowSummary | undefined)[] {
  const out = list.slice();
  for (const f of fresh.sort((a, b) => (before(a, b) ? -1 : 1))) {
    let i = out.length;
    while (i > 0 && out[i - 1] && before(f, out[i - 1]!)) i--;
    out.splice(i, 0, f);
  }
  return out;
}
