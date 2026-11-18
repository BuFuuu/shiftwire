// Flow details, fetched on demand and kept in a small cache so arrowing
// through the list is instant (neighbours are prefetched).

import type { FlowDetail } from './types';

const MAX_CACHED = 64;
const cache = new Map<string, Promise<FlowDetail | null>>();

export function getDetail(id: string): Promise<FlowDetail | null> {
  let hit = cache.get(id);
  if (hit) {
    cache.delete(id); // re-insert: Map order doubles as LRU order
  } else {
    hit = fetch(`/api/flows/${encodeURIComponent(id)}`)
      .then((r) => (r.ok ? (r.json() as Promise<FlowDetail>) : null))
      .catch(() => null);
    hit.then((d) => d === null && cache.delete(id)); // retry failures next time
  }
  cache.set(id, hit);
  while (cache.size > MAX_CACHED) cache.delete(cache.keys().next().value!);
  return hit;
}

export function prefetch(...ids: (string | undefined)[]): void {
  for (const id of ids) if (id && !cache.has(id)) void getDetail(id);
}

/** Forget a cached detail (its tags, color or note changed). */
export function invalidate(id: string): void {
  cache.delete(id);
}
