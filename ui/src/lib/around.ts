// The burst a request belongs to: HTTP has no id for "this click sent these
// requests", so the usual approximation is the run of requests by the same actor,
// the same way in, with no quiet gap longer than a limit (the server walks it:
// GET /api/flows/{id}/burst).

import { addPin, history, jumpTo } from './live.svelte';
import type { Burst, FlowSummary } from './types';

/** The gaps offered in the menu, in seconds. */
export const BURST_GAPS = [1, 2, 3, 10, 30];

export async function burstOf(f: FlowSummary, gapMs: number, signal?: AbortSignal): Promise<Burst | null> {
  const res = await fetch(`/api/flows/${encodeURIComponent(f.id)}/burst?gap_ms=${gapMs}`, { signal });
  return res.ok ? ((await res.json()) as Burst) : null;
}

/** The burst as a filter: its time span for that actor and way in. */
export const burstFilter = (b: Burst) => `actor:=${b.actor} via:${b.via} ts:>=${b.first_ts} ts:<=${b.last_ts}`;

/** How a History row draws the burst line: ┌ first member, ├ member, └ last, │ a row
 *  of someone else inside the burst's span. `selected`: the selected row's burst. */
export type Line = { kind: 'start' | 'mid' | 'end' | 'pass'; selected: boolean };

/** The burst lines for rows `from..to` (time order), with the same rule as the
 *  server: a row joins its actor's current burst unless it starts more than `gap`
 *  after everything in it ended. Rows not loaded yet cut the lines. */
export function burstLines(
  rows: (FlowSummary | undefined)[],
  from: number,
  to: number,
  gap: number,
  selectedId: string | null,
): Map<number, Line> {
  type Group = { first: number; last: number; members: number[]; end: number };
  const groups: Group[] = [];
  const open = new Map<string, Group>();
  let selected: Group | undefined;
  for (let i = Math.max(0, from); i < Math.min(to, rows.length); i++) {
    const f = rows[i];
    if (!f) {
      open.clear();
      continue;
    }
    const key = `${f.actor}\0${f.via}`;
    let g = open.get(key);
    if (!g || f.ts - g.end > gap) {
      g = { first: i, last: i, members: [], end: 0 };
      groups.push(g);
      open.set(key, g);
    }
    g.members.push(i);
    g.last = i;
    g.end = Math.max(g.end, f.ts + f.duration_ms);
    if (f.id === selectedId) selected = g;
  }
  const lines = new Map<number, Line>();
  const many = groups.filter((g) => g.members.length > 1);
  // The selected burst first: where spans overlap, its line wins.
  for (const g of selected && selected.members.length > 1 ? [selected, ...many] : many) {
    const isSelected = g === selected;
    for (const i of g.members) {
      const kind = i === g.first ? 'start' : i === g.last ? 'end' : 'mid';
      if (!lines.has(i) || isSelected) lines.set(i, { kind, selected: isSelected });
    }
    for (let i = g.first + 1; i < g.last; i++) if (!lines.has(i)) lines.set(i, { kind: 'pass', selected: isSelected });
  }
  return lines;
}

/** Show only the burst (a chip that replaces the last "show" jump), or hide it (a lasting
 *  hide chip). Uses the gap picked in the menu, which the History lines follow too. */
export async function filterBurst(f: FlowSummary, mode: 'show' | 'hide'): Promise<void> {
  const b = await burstOf(f, history.burstGap);
  if (!b) return;
  if (mode === 'show') jumpTo(burstFilter(b));
  else addPin('hide', burstFilter(b));
}
