<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import SplitPane from '../components/SplitPane.svelte';
  import VirtualList from '../components/VirtualList.svelte';
  import ContextMenu from '../components/ContextMenu.svelte';
  import PinBar from '../components/PinBar.svelte';
  import Detail from './Detail.svelte';
  import { history, live, loadOlder } from '../lib/live.svelte';
  import { colorFlows } from '../lib/api';
  import { openTab } from '../lib/repeater.svelte';
  import { prefetch } from '../lib/detail';
  import { bytes, statusClass, time } from '../lib/format';
  import { COLORS, type FlowSummary } from '../lib/types';

  type SortKey = 'n' | 'actor' | 'method' | 'host' | 'path' | 'status' | 'size' | 'duration_ms';
  const columns: { label: string; key: SortKey }[] = [
    { label: '#', key: 'n' },
    { label: 'Time', key: 'n' },
    { label: 'Actor', key: 'actor' },
    { label: 'Method', key: 'method' },
    { label: 'Host', key: 'host' },
    { label: 'Path', key: 'path' },
    { label: 'Status', key: 'status' },
    { label: 'Size', key: 'size' },
    { label: 'ms', key: 'duration_ms' },
  ];
  const ROW = 26;

  let sort = $state<{ key: SortKey; dir: 1 | -1 }>({ key: 'n', dir: 1 });
  /** The row shown in the detail pane. */
  let selected = $state<string | null>(null);
  /** All highlighted rows (Ctrl-click toggles, Shift-click extends); actions apply to these. */
  let picked = $state.raw(new Set<string>());
  let rangeStart: string | null = null;
  let menu = $state<{ x: number; y: number; focus: 'tag' | 'note' | null } | null>(null);
  let viewport: HTMLDivElement | undefined = $state();
  let follow = true; // stick to the newest row while scrolled to the bottom

  // `history.flows` is already in time order; other orders are a sorted copy.
  const view = $derived.by(() => {
    const flows = history.flows;
    if (sort.key === 'n') return sort.dir === 1 ? flows : flows.toReversed();
    const k = sort.key;
    const d = sort.dir;
    return flows.toSorted((a, b) => {
      const x = a[k] ?? -1;
      const y = b[k] ?? -1;
      return (x < y ? -1 : x > y ? 1 : 0) * d;
    });
  });
  const timeOrder = $derived(sort.key === 'n' && sort.dir === 1);

  function setSort(key: SortKey) {
    sort = sort.key === key ? { key, dir: sort.dir === 1 ? -1 : 1 } : { key, dir: 1 };
  }

  /** The request's own number in the project; filters never renumber rows. */
  const number = (f: FlowSummary) => f.seq;

  /** Double-click: mark purple (again: clear). The quickest "look at this later". */
  function toggleMark(f: FlowSummary) {
    void colorFlows([f.id], f.color === 'purple' ? null : 'purple');
  }

  function select(i: number, e?: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }) {
    const f = view[i];
    if (!f) return;
    const start = rangeStart ? view.findIndex((x) => x.id === rangeStart) : -1;
    if (e?.shiftKey && start >= 0) {
      const [a, b] = start < i ? [start, i] : [i, start];
      picked = new Set(view.slice(a, b + 1).map((x) => x.id));
    } else if (e?.metaKey || e?.ctrlKey) {
      const next = new Set(picked);
      if (!next.delete(f.id)) next.add(f.id);
      picked = next;
      rangeStart = f.id;
    } else {
      picked = new Set([f.id]);
      rangeStart = f.id;
    }
    selected = f.id;
    prefetch(view[i - 1]?.id, view[i + 1]?.id);
    reveal(i);
  }

  function pickedFlows(): FlowSummary[] {
    return view.filter((f) => picked.has(f.id));
  }

  function openMenu(x: number, y: number, focus: 'tag' | 'note' | null = null) {
    if (picked.size) menu = { x, y, focus };
  }

  function onContextMenu(e: MouseEvent, i: number) {
    e.preventDefault();
    if (!picked.has(view[i].id)) select(i);
    openMenu(e.clientX, e.clientY);
  }

  /** Menu opened from the keyboard: next to the selected row. */
  function openMenuAtSelection(focus: 'tag' | 'note') {
    const i = view.findIndex((f) => f.id === selected);
    if (!viewport || i < 0) return;
    const r = viewport.getBoundingClientRect();
    openMenu(r.left + 260, r.top + (i + 1) * ROW - viewport.scrollTop, focus);
  }

  /** Scroll just enough to show row `i`. */
  function reveal(i: number) {
    if (!viewport || i < 0) return;
    const top = i * ROW;
    if (top < viewport.scrollTop) viewport.scrollTop = top;
    else if (top + ROW > viewport.scrollTop + viewport.clientHeight) viewport.scrollTop = top + ROW - viewport.clientHeight;
  }

  // When the list changes height (detail pane opens, window resizes), keep the
  // newest row in view while following, otherwise keep the selection in view.
  $effect(() => {
    if (!viewport) return;
    const el = viewport;
    const observer = new ResizeObserver(() => {
      if (follow && timeOrder) el.scrollTop = el.scrollHeight;
      else if (selected) reveal(view.findIndex((f) => f.id === selected));
    });
    observer.observe(el);
    return () => observer.disconnect();
  });

  function onKey(e: KeyboardEvent) {
    const current = selected ? view.findIndex((f) => f.id === selected) : -1;
    const page = Math.max(1, Math.floor((viewport?.clientHeight ?? ROW) / ROW) - 1);
    const moves: Record<string, number> = {
      ArrowDown: current + 1,
      ArrowUp: Math.max(0, current - 1),
      PageDown: current + page,
      PageUp: Math.max(0, current - page),
      Home: 0,
      End: view.length - 1,
    };
    const ids = [...picked];
    if (e.key in moves) {
      e.preventDefault();
      select(Math.min(view.length - 1, moves[e.key]), { shiftKey: e.shiftKey, metaKey: false, ctrlKey: false });
    } else if (e.key === 'Escape') {
      selected = null;
      picked = new Set();
    } else if ((e.metaKey || e.ctrlKey) && e.key === 'a') {
      e.preventDefault();
      picked = new Set(view.map((f) => f.id));
    } else if (e.metaKey || e.ctrlKey || e.altKey || !ids.length) {
      return; // leave browser and app shortcuts alone
    } else if (e.key >= '1' && e.key <= '7') {
      void colorFlows(ids, COLORS[Number(e.key) - 1]);
    } else if (e.key === '0') {
      void colorFlows(ids, null);
    } else if (e.key === 'r' && selected) {
      e.preventDefault();
      const flow = view.find((f) => f.id === selected);
      if (flow && flow.method !== 'CONNECT') void openTab(flow.id);
    } else if (e.key === 't' || e.key === 'n') {
      e.preventDefault();
      openMenuAtSelection(e.key === 't' ? 'tag' : 'note');
    }
  }

  async function onScroll() {
    if (!viewport) return;
    follow = viewport.scrollTop + viewport.clientHeight >= viewport.scrollHeight - ROW;
    // Near the top in time order: fetch the previous page, keeping the view where it is.
    if (timeOrder && viewport.scrollTop < 30 * ROW) {
      const added = await loadOlder();
      if (added) viewport.scrollTop += added * ROW;
    }
  }

  $effect(() => {
    void view.length;
    if (follow && timeOrder && viewport) viewport.scrollTop = viewport.scrollHeight;
  });
</script>

{#snippet list()}
  <PinBar />
  <div class="head">
    {#each columns as c, i (i)}
      <button class:active={sort.key === c.key && c.label !== 'Time'} onclick={() => setSort(c.key)}>
        {c.label}{#if sort.key === c.key && c.label !== 'Time'}<span class="arrow">{sort.dir === 1 ? '▲' : '▼'}</span>{/if}
      </button>
    {/each}
  </div>
  {#if history.flows.length === 0}
    <div class="empty">
      <Icon name="radar" size={40} />
      <h2>Waiting for traffic</h2>
      {#if live.status?.proxy_error}
        <p class="problem">Proxy is down: {live.status.proxy_error}</p>
      {:else}
        <p>
          Point a browser, VM or AI tool at
          <code>{live.status?.proxy_addr ?? '127.0.0.1:8080'}</code>
          and requests appear here live.
        </p>
        <p class="hint">
          For HTTPS, trust the <a href="/api/ca.crt" download>shiftWire CA certificate</a>
          (or open <code>http://shiftwire/</code> through the proxy).
        </p>
      {/if}
    </div>
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="rows" tabindex="0" role="grid" aria-label="Requests" onkeydown={onKey}>
      <VirtualList items={view} key={(f) => f.id} rowHeight={ROW} bind:viewport onscroll={onScroll}>
        {#snippet row(f: FlowSummary, i: number)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row"
            class:odd={i % 2 === 1}
            class:picked={picked.has(f.id)}
            class:colored={f.color}
            style:--c={f.color ? `var(--c-${f.color})` : null}
            class:failed={f.error}
            role="row"
            tabindex="-1"
            onclick={(e) => select(i, e)}
            ondblclick={() => toggleMark(f)}
            oncontextmenu={(e) => onContextMenu(e, i)}
          >
            <span class="num">{number(f)}</span>
            <span class="num">{time(f.ts)}</span>
            <span><span class="actor" class:ai={f.actor !== 'proxy'}>{f.actor}</span></span>
            <span class="method">{f.method}</span>
            <span class="clip">{f.host}{f.port !== 80 && f.port !== 443 ? `:${f.port}` : ''}</span>
            <span class="pathcell">
              <span class="clip path" title={f.error ?? f.path}>
                {f.method === 'CONNECT' ? (f.error ? 'TLS not intercepted' : 'tunnel (passthrough)') : f.path}
              </span>
              {#if f.mark}<span class="mark sev-{f.mark}" title={f.mark_reason ?? ''}>★ {f.mark}</span>{/if}
              {#each f.tags as t (t)}<span class="tag">{t}</span>{/each}
              {#if f.note}<span class="note" title={f.note}>✎</span>{/if}
            </span>
            <span class="num status {statusClass(f.status)}">{f.status ?? 'ERR'}</span>
            <span class="num">{bytes(f.size)}</span>
            <span class="num">{f.duration_ms}</span>
          </div>
        {/snippet}
      </VirtualList>
    </div>
  {/if}
{/snippet}

{#snippet detail()}
  <Detail id={selected!} onclose={() => (selected = null)} />
{/snippet}

<section class="history">
  <SplitPane top={list} bottom={selected ? detail : undefined} />
</section>

{#if menu}
  <ContextMenu
    x={menu.x}
    y={menu.y}
    focus={menu.focus}
    flows={pickedFlows()}
    onclose={() => {
      menu = null;
      viewport?.parentElement?.focus();
    }}
  />
{/if}

<style>
  .history {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .rows {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    outline: none;
  }
  .head,
  .row {
    display: grid;
    grid-template-columns: var(--history-cols);
    gap: 12px;
    align-items: center;
    padding: 0 16px;
    white-space: nowrap;
  }
  .head {
    height: 30px;
    flex: none;
    overflow: hidden;
    scrollbar-gutter: stable;
    border-bottom: 1px solid var(--hairline);
  }
  .head button {
    border: 0;
    background: none;
    padding: 0;
    text-align: left;
    font-size: 11.5px;
    font-weight: 500;
    color: var(--text-2);
  }
  .head button:hover,
  .head button.active {
    color: var(--text);
  }
  .arrow {
    margin-left: 3px;
    font-size: 8px;
  }
  .row {
    height: 26px;
  }
  .row.odd {
    background: var(--stripe);
  }
  .row:hover {
    background: var(--hover);
  }
  .row.picked {
    background: var(--accent);
    color: #fff;
  }
  .row.picked .num,
  .row.picked .status,
  .row.picked .path {
    color: inherit;
  }
  .row.picked .actor,
  .row.picked .tag {
    color: #fff;
    background: rgba(255, 255, 255, 0.22);
  }
  .pathcell {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
  }
  .pathcell .path {
    flex: 0 1 auto;
  }
  .tag {
    flex: none;
    padding: 0 6px;
    border-radius: 8px;
    font-size: 11px;
    line-height: 16px;
    color: var(--accent);
    background: var(--selected);
  }
  .mark {
    flex: none;
    padding: 0 6px;
    border-radius: 8px;
    font-size: 11px;
    line-height: 16px;
    font-weight: 600;
    color: #fff;
    background: var(--sev);
  }
  .note {
    flex: none;
    font-size: 12px;
    color: var(--orange);
  }
  .num {
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
  }
  .clip {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .path {
    font-family: var(--mono);
    font-size: 12px;
  }
  .method {
    font-weight: 500;
  }
  .actor {
    padding: 1px 7px;
    border-radius: 9px;
    font-size: 11.5px;
    color: var(--text-2);
    background: var(--field);
  }
  .actor.ai {
    color: var(--accent);
    background: var(--selected);
  }
  .failed .path {
    color: var(--red);
  }
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-3);
    text-align: center;
  }
  h2 {
    margin: 10px 0 0;
    font-size: 17px;
    font-weight: 600;
    color: var(--text);
  }
  p {
    margin: 0;
    max-width: 380px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .hint {
    font-size: 12.5px;
  }
  a {
    color: var(--accent);
    text-decoration: none;
  }
  .problem {
    color: var(--red);
  }
</style>
