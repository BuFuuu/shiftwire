// Commands to the daemon. Updates are optimistic: the UI changes first, and the
// pushed event confirms it (or the change is rolled back on error).

import { history, live, replaceFlows } from './live.svelte';
import type { AgentMode, FlowSummary } from './types';

async function call(method: string, path: string, body?: unknown): Promise<void> {
  const res = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(await res.text());
}

export async function setAgentMode(mode: AgentMode): Promise<void> {
  const before = live.agentMode;
  live.agentMode = mode;
  try {
    await call('PUT', '/api/agent-mode', { mode });
  } catch {
    live.agentMode = before;
  }
}

/** Change rows locally right away; restore them if the server says no. */
async function optimistic(ids: string[], change: (f: FlowSummary) => FlowSummary, send: () => Promise<void>) {
  const wanted = new Set(ids);
  const before = history.rows.filter((f) => f !== undefined && wanted.has(f.id)) as FlowSummary[];
  replaceFlows(before.map(change));
  try {
    await send();
  } catch (e) {
    replaceFlows(before);
    console.warn('shiftWire: update failed', e);
  }
}

export function tagFlows(ids: string[], add: string[], remove: string[] = []) {
  return optimistic(
    ids,
    (f) => ({ ...f, tags: [...new Set([...f.tags, ...add])].filter((t) => !remove.includes(t)) }),
    () => call('POST', '/api/flows/tags', { ids, add, remove }),
  );
}

export function colorFlows(ids: string[], color: string | null) {
  return optimistic(ids, (f) => ({ ...f, color }), () => call('POST', '/api/flows/color', { ids, color }));
}

export function noteFlows(ids: string[], note: string) {
  const value = note.trim() || null;
  return optimistic(ids, (f) => ({ ...f, note: value }), () => call('POST', '/api/flows/note', { ids, note }));
}
