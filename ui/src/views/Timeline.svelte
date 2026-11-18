<script lang="ts">
  // What the agents did, grouped by plan: each intent note with the requests that
  // followed it, findings (marks) always visible, the operator's messages in between.
  import ContextMenu from '../components/ContextMenu.svelte';
  import SplitPane from '../components/SplitPane.svelte';
  import Detail from './Detail.svelte';
  import { timeline } from '../lib/live.svelte';
  import { nav } from '../lib/nav.svelte';
  import { repeater } from '../lib/repeater.svelte';
  import { bytes, statusClass, time } from '../lib/format';
  import { SEVERITIES, type FlowSummary, type TimelineEntry } from '../lib/types';

  type Item =
    | { kind: 'entry'; at: string; entry: TimelineEntry }
    | { kind: 'flow'; at: string; flow: FlowSummary }
    | { kind: 'more'; at: string; hidden: number; run: string }
    | { kind: 'less'; at: string; run: string };

  interface Group {
    key: string;
    /** The plan; null for what happened before the first one. */
    intent: TimelineEntry | null;
    items: Item[];
    flows: FlowSummary[];
    marks: TimelineEntry[];
    first: number;
    last: number;
  }

  /** Runs longer than this collapse to their first 5 and last 3 requests. */
  const RUN_LIMIT = 12;
  let expandedRuns = $state(new Set<string>());

  let agent = $state<string | null>(null);
  let search = $state('');
  let findingsOnly = $state(false);
  let newestFirst = $state(true);
  /** Which plans are open: the newest by default, or all / none after "Expand/Collapse all". */
  let openMode = $state<'latest' | 'all' | 'none'>('latest');
  let flipped = $state(new Set<string>());

  let selected = $state<string | null>(null);
  /** Right-click on a request: the same menu as in History. */
  let menu = $state<{ x: number; y: number; flow: FlowSummary } | null>(null);
  let message = $state('');
  let sending = $state(false);
  let scroller: HTMLDivElement | undefined = $state();

  // Opening the Timeline counts as having seen the marks.
  $effect(() => {
    timeline.unseenMarks = 0;
  });

  const agents = $derived([...new Set([...timeline.entries, ...timeline.flows].map((x) => x.actor))].filter((a) => a !== 'human'));

  const matches = (f: FlowSummary, q: string) =>
    `#${f.seq} ${f.method} ${f.status ?? 'err'} ${f.host}${f.path} ${f.tags.join(' ')} ${f.mark ?? ''}`.toLowerCase().includes(q);

  // Entries and flows share one id space (time-ordered ULIDs), so sorting by id interleaves them.
  const groups = $derived.by(() => {
    const keep = (actor: string) => !agent || actor === agent || actor === 'human';
    const all: Item[] = [
      ...timeline.entries.filter((e) => keep(e.actor)).map((entry) => ({ kind: 'entry' as const, at: entry.id, entry })),
      ...timeline.flows.filter((f) => keep(f.actor)).map((flow) => ({ kind: 'flow' as const, at: flow.id, flow })),
    ];
    all.sort((a, b) => (a.at < b.at ? -1 : 1));
    const out: Group[] = [];
    const byKey = new Map<string, Group>();
    let g: Group = { key: 'start', intent: null, items: [], flows: [], marks: [], first: 0, last: 0 };
    const ts = (i: Item) => (i.kind === 'entry' ? i.entry.ts : i.kind === 'flow' ? i.flow.ts : 0);
    for (const item of all) {
      if (item.kind === 'entry' && item.entry.kind === 'intent' && !isPrepared(item.entry)) {
        if (g.items.length || g.intent) out.push(g);
        g = { key: item.at, intent: item.entry, items: [], flows: [], marks: [], first: item.entry.ts, last: item.entry.ts };
        byKey.set(g.key, g);
        continue;
      }
      // A request goes to the plan it was sent under, even when agents work in parallel.
      const target = (item.kind === 'flow' && item.flow.intent && byKey.get(item.flow.intent)) || g;
      target.items.push(item);
      if (item.kind === 'flow') target.flows.push(item.flow);
      if (item.kind === 'entry' && item.entry.kind === 'mark') target.marks.push(item.entry);
      target.first ||= ts(item);
      target.last = Math.max(target.last, ts(item));
    }
    if (g.items.length || g.intent) out.push(g);
    return out;
  });

  /** What each plan shows, after the filter; long runs of requests collapse. */
  const view = $derived.by(() => {
    const q = search.trim().toLowerCase();
    const shown = groups.flatMap((g) => {
      const planHit = !!q && !!g.intent?.text.toLowerCase().includes(q);
      let items = g.items.filter((i) => {
        if (i.kind !== 'flow') return true;
        if (findingsOnly) return false;
        return !q || planHit || matches(i.flow, q);
      });
      if (q && !planHit && !items.some((i) => i.kind === 'flow') && !g.marks.some((m) => m.text.toLowerCase().includes(q))) return [];
      items = collapseRuns(items);
      return [{ ...g, items }];
    });
    return newestFirst ? shown.reverse() : shown;
  });

  function collapseRuns(items: Item[]): Item[] {
    const out: Item[] = [];
    let run: Item[] = [];
    const flush = () => {
      const key = run[0]?.at ?? '';
      if (run.length > RUN_LIMIT) {
        if (expandedRuns.has(key)) out.push(...run, { kind: 'less', at: key + '-less', run: key });
        else out.push(...run.slice(0, 5), { kind: 'more', at: key + '-more', hidden: run.length - 8, run: key }, ...run.slice(-3));
      } else out.push(...run);
      run = [];
    };
    for (const item of items) {
      if (item.kind === 'flow') run.push(item);
      else {
        flush();
        out.push(item);
      }
    }
    flush();
    return out;
  }

  /** A request an agent prepared in the Repeater (older versions logged it as an intent). */
  const isPrepared = (e: TimelineEntry) => e.kind === 'prepared' || (e.kind === 'intent' && e.text.startsWith('Prepared "'));
  function openPrepared(e: TimelineEntry) {
    const name = /"(.*)"/.exec(e.text)?.[1];
    const tab = repeater.tabs.find((t) => t.name === `${e.actor}: ${name}`);
    if (tab) nav.repeaterTab = tab.id;
    nav.view = 'repeater';
  }

  const latestKey = $derived(groups.at(-1)?.key);
  function isOpen(g: Group): boolean {
    if (search.trim()) return true; // searching: show where the hits are
    const base = openMode === 'all' || (openMode === 'latest' && g.key === latestKey);
    return flipped.has(g.key) ? !base : base;
  }
  function toggle(g: Group) {
    const next = new Set(flipped);
    if (!next.delete(g.key)) next.add(g.key);
    flipped = next;
  }
  function setAll(mode: 'all' | 'none') {
    openMode = mode;
    flipped = new Set();
  }
  const setRun = (run: string, open: boolean) => {
    const next = new Set(expandedRuns);
    if (open) next.add(run);
    else next.delete(run);
    expandedRuns = next;
  };

  /** Requests by response class, for a plan's summary. */
  function classes(flows: FlowSummary[]) {
    const c = { ok: 0, redirect: 0, client: 0, server: 0, failed: 0 };
    for (const f of flows) {
      const k = f.status == null ? 'failed' : (statusClass(f.status) as keyof typeof c);
      if (k in c) c[k]++;
    }
    return c;
  }
  const sevCounts = (marks: TimelineEntry[]) =>
    [...SEVERITIES].reverse().map((s) => ({ s, n: marks.filter((m) => m.severity === s).length })).filter((x) => x.n);

  const summary = $derived.by(() => {
    const flows = groups.flatMap((g) => g.flows);
    const c = classes(flows);
    return {
      plans: groups.filter((g) => g.intent).length,
      requests: flows.length,
      errors: c.server + c.failed,
      sevs: sevCounts(groups.flatMap((g) => g.marks)),
    };
  });

  const span = (g: Group) => (g.first && g.last && time(g.first) !== time(g.last) ? `${time(g.first)}–${time(g.last)}` : g.first ? time(g.first) : '');

  // Oldest first reads like a chat: stay at the newest item unless the user scrolled up.
  let atBottom = true;
  $effect(() => {
    void view.length;
    if (!newestFirst && atBottom && scroller) scroller.scrollTop = scroller.scrollHeight;
  });

  async function send() {
    const text = message.trim();
    if (!text || sending) return;
    sending = true;
    const res = await fetch('/api/timeline/message', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text }),
    });
    sending = false;
    if (res.ok) message = '';
  }
</script>

{#snippet flowRow(f: FlowSummary)}
  <button
    class="flow"
    class:on={selected === f.id}
    class:failed={f.error}
    class:colored={f.color}
    style:--c={f.color ? `var(--c-${f.color})` : null}
    onclick={() => (selected = f.id)}
    oncontextmenu={(e) => (e.preventDefault(), (selected = f.id), (menu = { x: e.clientX, y: e.clientY, flow: f }))}
    title={f.error ?? ''}
  >
    <span class="seq">#{f.seq}</span>
    <span class="when">{time(f.ts)}</span>
    <span class="method">{f.method}</span>
    <span class="status {statusClass(f.status)}">{f.status ?? 'ERR'}</span>
    <span class="path">{f.host}{f.path}</span>
    {#if f.mark}<span class="badge sev-{f.mark}">★ {f.mark}</span>{/if}
    {#each f.tags as t (t)}<span class="tag">{t}</span>{/each}
    <span class="size">{bytes(f.size)}</span>
  </button>
{/snippet}

{#snippet markRow(m: TimelineEntry)}
  <button class="markline sev-{m.severity}" onclick={() => (selected = m.flow_id)}>
    <span class="sev">★ {m.severity}</span>
    <span class="mtext"><b>{m.actor}</b>: {m.text}</span>
    <span class="when">{time(m.ts)}</span>
  </button>
{/snippet}

{#snippet feed()}
  <div class="bar">
    <div class="chips">
      <button class:on={!agent} onclick={() => (agent = null)}>All agents</button>
      {#each agents as a (a)}
        <button class:on={agent === a} onclick={() => (agent = a)}>{a}</button>
      {/each}
    </div>
    <input class="search" bind:value={search} placeholder="Filter: path, status, tag, #…" spellcheck="false" />
    <div class="seg" role="radiogroup" aria-label="Show">
      <button class:on={!findingsOnly} onclick={() => (findingsOnly = false)} title="Plans, requests and findings">Everything</button>
      <button class:on={findingsOnly} onclick={() => (findingsOnly = true)} title="Only plans, findings and messages">Findings</button>
    </div>
    <button class="plain" onclick={() => (newestFirst = !newestFirst)} title="Change the order">
      {newestFirst ? 'Newest first ↓' : 'Oldest first ↑'}
    </button>
    <button class="plain" onclick={() => setAll(openMode === 'all' ? 'none' : 'all')}>
      {openMode === 'all' ? 'Collapse all' : 'Expand all'}
    </button>
  </div>

  {#if groups.length}
    <div class="summary">
      <span><b>{summary.plans}</b> plan{summary.plans === 1 ? '' : 's'}</span>
      <span><b>{summary.requests.toLocaleString()}</b> requests</span>
      {#if summary.errors}<span class="err"><b>{summary.errors}</b> server errors or failures</span>{/if}
      {#each summary.sevs as { s, n } (s)}<span class="badge sev-{s}">★ {n} {s}</span>{/each}
    </div>
  {/if}

  <div class="feed" bind:this={scroller} onscroll={() => scroller && (atBottom = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 40)}>
    {#if groups.length === 0}
      <div class="empty">
        <h2>No agent activity yet</h2>
        <p>Connect an agent over MCP (see Settings). Its plans, requests and findings appear here as they happen.</p>
      </div>
    {:else if view.length === 0}
      <p class="none">Nothing matches.</p>
    {/if}
    {#each view as g (g.key)}
      {@const open = isOpen(g)}
      {@const c = classes(g.flows)}
      <section class="group" class:open>
        <button class="head" onclick={() => toggle(g)} aria-expanded={open}>
          <svg class="chev" class:open width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6" /></svg>
          <span class="who">{g.intent?.actor ?? ''}</span>
          <span class="plan" class:muted={!g.intent}>{g.intent?.text ?? 'Before the first plan'}</span>
          <span class="stats">
            {#if g.flows.length}<span class="n">{g.flows.length} req</span>{/if}
            {#if c.ok}<span class="status ok">{c.ok}</span>{/if}
            {#if c.redirect}<span class="status redirect">{c.redirect}</span>{/if}
            {#if c.client}<span class="status client">{c.client}</span>{/if}
            {#if c.server + c.failed}<span class="status server">{c.server + c.failed}</span>{/if}
            {#each sevCounts(g.marks) as { s, n } (s)}<span class="badge sev-{s}">★ {n}</span>{/each}
          </span>
          <span class="when">{span(g)}</span>
        </button>
        <div class="body">
          {#each g.items as item (item.at)}
            {#if item.kind === 'entry' && item.entry.kind === 'mark'}
              {@render markRow(item.entry)}
            {:else if item.kind === 'entry' && isPrepared(item.entry)}
              {#if open}
                <button class="prepared" onclick={() => openPrepared(item.entry)} title="Open it in the Repeater">
                  <span class="pi">↗</span> {item.entry.text.replace(' for you to review and send.', '')}
                  <span class="when">{time(item.entry.ts)}</span>
                </button>
              {/if}
            {:else if item.kind === 'entry'}
              {#if open}
                <div class="operator"><span class="when">{time(item.entry.ts)} · you</span> {item.entry.text}</div>
              {/if}
            {:else if !open}
              <!-- collapsed: only findings show -->
            {:else if item.kind === 'more'}
              <button class="more" onclick={() => setRun(item.run, true)}>… {item.hidden.toLocaleString()} more requests</button>
            {:else if item.kind === 'less'}
              <button class="more" onclick={() => setRun(item.run, false)}>Show fewer</button>
            {:else if item.kind === 'flow'}
              {@render flowRow(item.flow)}
            {/if}
          {/each}
        </div>
      </section>
    {/each}
  </div>

  <form class="composer" onsubmit={(e) => (e.preventDefault(), send())}>
    <input bind:value={message} placeholder="Message to the agents (they get it with their next action)…" spellcheck="false" />
    <button type="submit" disabled={!message.trim() || sending}>Send</button>
  </form>
{/snippet}

{#snippet detail()}
  <Detail id={selected!} onclose={() => (selected = null)} />
{/snippet}

<section class="timeline">
  <SplitPane top={feed} bottom={selected ? detail : undefined} />
</section>

{#if menu}
  <ContextMenu x={menu.x} y={menu.y} flows={[menu.flow]} onclose={() => (menu = null)} />
{/if}

<style>
  .timeline {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 7px 16px;
    border-bottom: 1px solid var(--hairline);
  }
  .chips {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .chips button,
  .plain {
    border: 1px solid var(--hairline);
    border-radius: 999px;
    padding: 2px 11px;
    background: none;
    color: var(--text-2);
    font-size: 12px;
    white-space: nowrap;
  }
  .chips button.on {
    background: var(--selected);
    border-color: transparent;
    color: var(--accent);
  }
  .plain:hover {
    color: var(--text);
  }
  .search {
    flex: 1;
    min-width: 160px;
    height: 26px;
    padding: 0 10px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: var(--field);
    color: var(--text);
    font: 12px var(--mono);
    outline: none;
  }
  .search:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  .seg {
    display: inline-flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--field);
  }
  .seg button {
    padding: 1px 10px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text-2);
    font-size: 12px;
  }
  .seg button.on {
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
  }
  .summary {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 12px;
    padding: 6px 16px;
    border-bottom: 1px solid var(--hairline);
    font-size: 12px;
    color: var(--text-2);
  }
  .summary b {
    color: var(--text);
  }
  .summary .err b {
    color: var(--red);
  }
  .feed {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 6px 12px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .group {
    border-radius: 8px;
  }
  .group.open {
    background: color-mix(in srgb, var(--field) 50%, transparent);
    padding-bottom: 4px;
  }
  .head {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    border: 0;
    border-left: 3px solid var(--accent);
    border-radius: 6px;
    background: none;
    color: var(--text);
    text-align: left;
    font-size: 13px;
  }
  .head:hover {
    background: var(--hover);
  }
  .chev {
    flex: none;
    color: var(--text-3);
    transition: transform 140ms ease;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .who {
    flex: none;
    font-weight: 600;
    color: var(--accent);
  }
  .plan {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .group.open .plan {
    white-space: normal;
  }
  .plan.muted {
    color: var(--text-3);
  }
  .stats {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .stats .n {
    color: var(--text-2);
  }
  .body {
    display: flex;
    flex-direction: column;
    padding-left: 18px;
  }
  .when {
    font-size: 11.5px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .flow {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 22px;
    padding: 0 8px;
    border: 0;
    border-radius: 5px;
    background: none;
    color: var(--text);
    text-align: left;
    font-size: 12px;
    white-space: nowrap;
  }
  .flow:hover {
    background: var(--hover);
  }
  .flow.on {
    background: var(--selected);
  }
  .seq {
    width: 34px;
    color: var(--text-3);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .method {
    font-weight: 500;
    width: 52px;
  }
  .flow .status {
    width: 30px;
    font-variant-numeric: tabular-nums;
  }
  .failed .path {
    color: var(--red);
  }
  .path {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font: 11.5px var(--mono);
  }
  .size {
    margin-left: auto;
    color: var(--text-3);
    font-size: 11px;
  }
  .tag {
    padding: 0 6px;
    border-radius: 8px;
    font-size: 10.5px;
    color: var(--accent);
    background: var(--selected);
  }
  .badge,
  .sev {
    padding: 0 6px;
    border-radius: 8px;
    font-size: 11px;
    font-weight: 600;
    color: #fff;
    background: var(--sev);
  }
  .more {
    margin: 2px 0 2px 8px;
    padding: 1px 10px;
    align-self: flex-start;
    border: 1px dashed var(--hairline);
    border-radius: 999px;
    background: none;
    color: var(--text-2);
    font-size: 11.5px;
  }
  .more:hover {
    color: var(--text);
  }
  .markline {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 2px 0;
    padding: 4px 10px;
    border: 0;
    border-radius: 7px;
    background: color-mix(in srgb, var(--sev) 12%, transparent);
    color: var(--text);
    text-align: left;
    font-size: 12.5px;
  }
  .mtext {
    flex: 1;
    min-width: 0;
  }
  .prepared {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 24px;
    padding: 0 8px;
    border: 0;
    border-radius: 5px;
    background: none;
    color: var(--ic-repeater);
    text-align: left;
    font-size: 12px;
  }
  .prepared:hover {
    background: var(--hover);
  }
  .prepared .when {
    margin-left: auto;
  }
  .pi {
    font-weight: 700;
  }
  .operator {
    align-self: flex-end;
    max-width: 70%;
    margin: 4px 0;
    padding: 5px 11px;
    border-radius: 11px;
    background: var(--accent);
    color: #fff;
    font-size: 12.5px;
  }
  .operator .when {
    color: rgba(255, 255, 255, 0.75);
  }
  .composer {
    display: flex;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid var(--hairline);
  }
  .composer input {
    flex: 1;
    height: 32px;
    padding: 0 12px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: var(--field);
    color: var(--text);
    font: inherit;
    outline: none;
  }
  .composer input:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  .composer button {
    border: 0;
    border-radius: 8px;
    padding: 0 16px;
    background: var(--accent);
    color: #fff;
    font-weight: 500;
  }
  .none {
    margin: 20px auto;
    color: var(--text-3);
  }
  .empty {
    margin: auto;
    text-align: center;
  }
  .empty h2 {
    margin: 0 0 6px;
    font-size: 17px;
  }
  .empty p {
    margin: 0;
    color: var(--text-2);
    max-width: 420px;
  }
</style>
