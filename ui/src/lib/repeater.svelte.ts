// Repeater tabs live on the daemon (they survive reloads, and agents can use
// them later); every change is pushed to all windows as `repeater_changed`.

import { nav } from './nav.svelte';
import type { FlowDetail, RepeaterTab, Target } from './types';

export const repeater = $state({ tabs: [] as RepeaterTab[] });

async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(await res.text());
  return res.status === 204 ? (undefined as T) : ((await res.json()) as T);
}

export async function loadTabs(): Promise<void> {
  repeater.tabs = await api<RepeaterTab[]>('GET', '/api/repeater/tabs');
}

/** New tab (a copy of a captured request, or blank), brought to the front. */
export async function openTab(fromFlow?: string): Promise<void> {
  const tab = await api<RepeaterTab>('POST', '/api/repeater/tabs', { from_flow: fromFlow ?? null });
  if (!repeater.tabs.some((t) => t.id === tab.id)) repeater.tabs.push(tab);
  nav.repeaterTab = tab.id;
  nav.view = 'repeater';
}

export function updateTab(id: string, edit: { name?: string; target?: Target; raw?: string }): Promise<RepeaterTab> {
  return api('PUT', `/api/repeater/tabs/${id}`, edit);
}

export async function closeTab(id: string): Promise<void> {
  const i = repeater.tabs.findIndex((t) => t.id === id);
  repeater.tabs = repeater.tabs.filter((t) => t.id !== id);
  if (nav.repeaterTab === id) nav.repeaterTab = (repeater.tabs[i] ?? repeater.tabs.at(-1))?.id ?? null;
  await api('DELETE', `/api/repeater/tabs/${id}`);
}

/** Saves the editor text and target, sends (following redirects if asked), returns the last response. */
export function sendTab(id: string, raw: string, target: Target, follow: boolean): Promise<FlowDetail & { redirects?: number }> {
  return api('POST', `/api/repeater/tabs/${id}/send`, { raw, target, follow });
}

export async function rawOf(flowId: string): Promise<string> {
  return (await api<{ raw: string }>('GET', `/api/flows/${flowId}/raw`)).raw;
}
