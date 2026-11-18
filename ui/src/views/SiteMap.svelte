<script lang="ts">
  // Hosts and paths seen so far, as a tree. IDs are collapsed by the server
  // (/users/{id}). Click a node to see its endpoints, an endpoint to see its
  // requests, a request to see it in full. "Show in History" filters there.
  import { hasScope, jumpTo, live, setScopeOnly } from '../lib/live.svelte';
  import ContextMenu from '../components/ContextMenu.svelte';
  import SplitPane from '../components/SplitPane.svelte';
  import Detail from './Detail.svelte';
  import { bytes, statusClass, time } from '../lib/format';
  import type { FlowSummary } from '../lib/types';

  interface Endpoint {
    scheme: string;
    host: string;
    port: number;
    path: string;
    methods: string[];
    count: number;
    ok: number;
    redirect: number;
    client_error: number;
    server_error: number;
    failed: number;
    by_agents: number;
    marked: boolean;
    in_scope: boolean;
    /** At least one response other than 404: it exists. */
    found: boolean;
  }
  interface Node {
    key: string;
    name: string;
    host: string;
    /** Path prefix of this node ('' for a host). */
    prefix: string;
    children: Node[];
    endpoints: Endpoint[];
    count: number;
    agents: number;
    marked: boolean;
    /** Something under it is in scope (Settings). */
    inScope: boolean;
  }

  let endpoints = $state.raw<Endpoint[]>([]);
  let selected = $state<string | null>(null);
  let open = $state(new Set<string>());
  let search = $state('');
  // "In scope" is one setting shared with History.
  const scopeOnly = $derived(live.scopeOnly && hasScope());
  let lastCount = -1;

  async function load() {
    endpoints = await fetch('/api/sitemap').then((r) => r.json());
  }

  // Refresh while open, but only when traffic arrived since the last load.
  $effect(() => {
    void load();
    const timer = setInterval(() => {
      const n = live.status?.flows ?? 0;
      if (n !== lastCount) {
        lastCount = n;
        void load();
      }
    }, 3000);
    return () => clearInterval(timer);
  });

  /** File type by extension: a color class and a short badge for the icon. */
  const TYPES: [string, string, string[]][] = [
    ['markup', '<>', ['html', 'htm', 'xhtml', 'php', 'asp', 'aspx', 'jsp', 'cgi', 'shtml']],
    ['script', 'JS', ['js', 'mjs', 'cjs', 'ts', 'jsx', 'tsx', 'map', 'wasm']],
    ['style', '#', ['css', 'scss', 'less']],
    ['data', '{}', ['json', 'xml', 'yaml', 'yml', 'csv', 'rss', 'atom', 'webmanifest']],
    ['image', '', ['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'ico', 'bmp', 'avif']],
    ['font', 'Aa', ['woff', 'woff2', 'ttf', 'otf', 'eot']],
    ['text', '≡', ['txt', 'md', 'log', 'pdf']],
    ['config', '⚙', ['env', 'ini', 'conf', 'config', 'cfg', 'bak', 'old', 'sql', 'htaccess', 'htpasswd', 'pem', 'key', 'swp', 'git']],
  ];
  function fileType(name: string): { cls: string; label: string } {
    const ext = name.includes('.') ? name.split('.').pop()!.toLowerCase() : '';
    const hit = TYPES.find(([, , exts]) => exts.includes(ext));
    return hit ? { cls: hit[0], label: hit[1] } : { cls: 'other', label: '' };
  }

  const authority = (e: Endpoint) =>
    `${e.scheme}://${e.host}${(e.scheme === 'https' && e.port === 443) || (e.scheme === 'http' && e.port === 80) ? '' : `:${e.port}`}`;

  const tree = $derived.by(() => {
    const q = search.trim().toLowerCase();
    const roots = new Map<string, Node>();
    const make = (key: string, name: string, host: string, prefix: string): Node => ({
      key, name, host, prefix, children: [], endpoints: [], count: 0, agents: 0, marked: false, inScope: false,
    });
    for (const e of endpoints) {
      if (!e.found) continue; // only 404s (or no answer): it does not exist, keep the tree clean
      if (scopeOnly && !e.in_scope) continue;
      const base = authority(e);
      if (q && !(base + e.path).toLowerCase().includes(q)) continue;
      let node = roots.get(base) ?? make(base, base, e.host, '');
      roots.set(base, node);
      const chain = [node];
      let prefix = '';
      for (const seg of e.path.split('/').filter(Boolean)) {
        prefix += '/' + seg;
        let child = node.children.find((c) => c.name === seg);
        if (!child) {
          child = make(base + prefix, seg, e.host, prefix);
          node.children.push(child);
        }
        node = child;
        chain.push(node);
      }
      node.endpoints.push(e);
      for (const n of chain) {
        n.count += e.count;
        n.agents += e.by_agents;
        n.marked ||= e.marked;
        n.inScope ||= e.in_scope;
      }
    }
    // Folders before files, then most requests first (alphabetical among equals).
    const sort = (nodes: Node[]) => {
      const folder = (n: Node) => (n.children.length ? 0 : 1);
      nodes.sort((a, b) => folder(a) - folder(b) || b.count - a.count || a.name.localeCompare(b.name));
      nodes.forEach((n) => sort(n.children));
      return nodes;
    };
    return sort([...roots.values()]);
  });

  /** Depth-first, only through open nodes: the rows of the tree. */
  const rows = $derived.by(() => {
    const out: { node: Node; depth: number }[] = [];
    const expanded = (n: Node) => open.has(n.key) || (search.trim() !== '' && n.children.length > 0);
    const walk = (nodes: Node[], depth: number) => {
      for (const n of nodes) {
        out.push({ node: n, depth });
        if (expanded(n)) walk(n.children, depth + 1);
      }
    };
    walk(tree, 0);
    return out;
  });

  const find = (nodes: Node[], key: string): Node | undefined => {
    for (const n of nodes) {
      if (n.key === key) return n;
      const hit = find(n.children, key);
      if (hit) return hit;
    }
  };
  const current = $derived(selected ? find(tree, selected) : undefined);

  const under = (n: Node): Endpoint[] => [...n.endpoints, ...n.children.flatMap(under)];
  const listed = $derived(current ? under(current).sort((a, b) => b.count - a.count) : []);

  function toggle(n: Node) {
    const next = new Set(open);
    if (!next.delete(n.key)) next.add(n.key);
    open = next;
  }

  /** `{id}` and friends become wildcards for the History filter. */
  const glob = (path: string) => path.replace(/\{(id|uuid|hash)\}/g, '*');

  // The endpoint clicked in the table, its requests, and the one shown in full.
  let picked = $state<string | null>(null);
  let requests = $state.raw<FlowSummary[]>([]);
  let flow = $state<string | null>(null);
  let loading = $state(false);
  let pickedEndpoint: Endpoint | null = null;
  /** Right-click on a request: the same menu as in History. */
  let menu = $state<{ x: number; y: number; flow: FlowSummary } | null>(null);

  /** Requests to exactly this endpoint (any query string), newest last. */
  async function pick(e: Endpoint) {
    picked = authority(e) + e.path;
    pickedEndpoint = e;
    loading = true;
    const p = glob(e.path);
    const filter = `host:=${e.host} (path:=${p} OR path:=${p}?*)`;
    const res = await fetch(`/api/flows?limit=200&filter=${encodeURIComponent(filter)}`);
    requests = res.ok ? await res.json() : [];
    loading = false;
    flow = requests.at(-1)?.id ?? null; // the newest, so request and response show at once
  }

  // A different node in the tree: start over.
  $effect(() => {
    void selected;
    picked = flow = null;
    requests = [];
  });

  function showInHistory(host: string, prefix: string) {
    const path = prefix ? glob(prefix) : '';
    jumpTo(path ? `host:=${host} path:=${path}*` : `host:=${host}`, `${host}${path}${path ? '*' : ''}`);
  }
</script>

<section class="sitemap">
  <aside class="tree">
    <div class="tools">
      <input class="search" bind:value={search} placeholder="Find a host or path…" spellcheck="false" />
      <button
        class="scope-only"
        class:on={scopeOnly}
        onclick={() => setScopeOnly(!live.scopeOnly)}
        disabled={!hasScope()}
        title={hasScope() ? 'Show only hosts in scope (also applies to History)' : 'Set a scope in Settings first'}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="4" /></svg>
        In scope
      </button>
    </div>
    {#if rows.length === 0}
      <p class="empty">
        {scopeOnly ? 'Nothing in scope has been seen yet.' : endpoints.length ? 'Nothing matches.' : 'No traffic yet. Endpoints appear here as requests are captured.'}
      </p>
    {/if}
    {#each rows as { node, depth } (node.key)}
      {@const isOpen = node.children.length > 0 && (open.has(node.key) || search.trim() !== '')}
      <button
        class="node"
        class:root={depth === 0}
        class:on={selected === node.key}
        class:scope={depth === 0 && node.inScope}
        title={depth === 0 && node.inScope ? 'In scope' : undefined}
        style:padding-left="{8 + depth * 16}px"
        onclick={() => (selected = node.key)}
        ondblclick={() => toggle(node)}
      >
        <!-- Guide lines show the hierarchy: one per level, plus an elbow into this node. -->
        {#each { length: depth } as _, i (i)}<span class="guide" style:left="{15 + i * 16}px"></span>{/each}
        {#if depth > 0}<span class="elbow" style:left="{15 + (depth - 1) * 16}px"></span>{/if}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <span class="chev" class:open={isOpen} class:leaf={!node.children.length} role="button" tabindex="-1"
          onclick={(e) => (e.stopPropagation(), toggle(node))}>
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6" /></svg>
        </span>
        <span class="icon" class:host={depth === 0}>
          {#if depth === 0}
            {#if node.key.startsWith('https')}
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="11" width="14" height="10" rx="2" /><path d="M8 11V8a4 4 0 0 1 8 0v3" /></svg>
            {:else}
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" /></svg>
            {/if}
          {:else if node.children.length}
            {#if isOpen}
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v1H7.5a2 2 0 0 0-1.9 1.4L3 19z" /><path d="M5.6 11.4A2 2 0 0 1 7.5 10H21l-2.6 7.6a2 2 0 0 1-1.9 1.4H3" /></svg>
            {:else}
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
            {/if}
          {:else}
            {@const t = fileType(node.name)}
            {#if t.cls === 'image'}
              <svg class="ft image" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2.5" /><circle cx="9" cy="10" r="1.6" /><path d="M21 16l-5-5-9 9" /></svg>
            {:else}
              <svg class="ft {t.cls}" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round">
                <path d="M6 2.5h8.5L19 7v14.5H6z" /><path d="M14.5 2.5V7H19" />
                {#if t.label}<text x="12.5" y="18" text-anchor="middle" font-size="7.5" font-weight="700" fill="currentColor" stroke="none" font-family="system-ui">{t.label}</text>{/if}
              </svg>
            {/if}
          {/if}
        </span>
        <span class="name" class:host={depth === 0}>{depth === 0 ? node.name.replace(/^https?:\/\//, '') : node.name}</span>
        {#if node.marked}<span class="star" title="Something here was marked important">★</span>{/if}
        {#if node.agents}<span class="ai" title="{node.agents} requests by AI agents">AI</span>{/if}
        <span class="count">{node.count.toLocaleString()}</span>
      </button>
    {/each}
  </aside>

  <div class="detail">
    <SplitPane top={overview} bottom={flow ? full : undefined} />
  </div>
</section>

{#if menu}
  <ContextMenu
    x={menu.x}
    y={menu.y}
    flows={[menu.flow]}
    onclose={() => {
      menu = null;
      // Colors and tags may have changed: reload the list (the server has them a moment later).
      setTimeout(() => pickedEndpoint && pick(pickedEndpoint), 200);
    }}
  />
{/if}

{#snippet full()}
  <Detail id={flow!} onclose={() => (flow = null)} />
{/snippet}

{#snippet overview()}
    {#if current}
      <header>
        <h2>{current.prefix ? current.prefix : current.name}</h2>
        <span class="sub">{current.prefix ? current.host : ''} · {listed.length} endpoint{listed.length === 1 ? '' : 's'} · {current.count.toLocaleString()} requests</span>
        <button class="primary" onclick={() => showInHistory(current!.host, current!.prefix)}>Show in History</button>
      </header>
      <div class="table">
        <div class="tr th"><span>Methods</span><span>Endpoint</span><span>Requests</span><span>Responses</span><span></span></div>
        {#each listed as e (authority(e) + e.path)}
          <button class="tr" class:on={picked === authority(e) + e.path} onclick={() => pick(e)} title="Show this endpoint's requests">
            <span class="methods">{e.methods.join(' ')}</span>
            <span class="path">{e.path}</span>
            <span class="num">{e.count.toLocaleString()}</span>
            <span class="classes">
              <!-- Plain counts per response class: informative, not alarming. -->
              {#if e.ok}<i title="Successful">2xx {e.ok}</i>{/if}
              {#if e.redirect}<i title="Redirects">3xx {e.redirect}</i>{/if}
              {#if e.client_error}<i title="Client errors">4xx {e.client_error}</i>{/if}
              {#if e.server_error}<i title="Server errors">5xx {e.server_error}</i>{/if}
              {#if e.failed}<i title="No response">failed {e.failed}</i>{/if}
            </span>
            <span class="flags">
              {#if e.marked}<span class="star">★</span>{/if}
              {#if e.by_agents}<span class="ai">AI {e.by_agents}</span>{/if}
            </span>
          </button>
          {#if picked === authority(e) + e.path}
            <div class="requests">
              {#each requests as r (r.id)}
                <button
                  class="req"
                  class:on={flow === r.id}
                  class:colored={r.color}
                  style:--c={r.color ? `var(--c-${r.color})` : null}
                  onclick={() => (flow = r.id)}
                  oncontextmenu={(e) => (e.preventDefault(), (flow = r.id), (menu = { x: e.clientX, y: e.clientY, flow: r }))}
                >
                  <span class="seq">#{r.seq}</span>
                  <span class="when">{time(r.ts)}</span>
                  <span class="m">{r.method}</span>
                  <span class="p">{r.path}</span>
                  <span class="status {statusClass(r.status)}">{r.status ?? 'ERR'}</span>
                  <span class="size">{bytes(r.size)}</span>
                  {#if r.mark || r.tags.length}
                    <span class="meta-tags">
                      {#if r.mark}<span class="badge sev-{r.mark}">★ {r.mark}</span>{/if}
                      {#each r.tags as t (t)}<span class="tag">{t}</span>{/each}
                    </span>
                  {/if}
                </button>
              {:else}
                <p class="none">{loading ? 'Loading…' : 'No stored requests match.'}</p>
              {/each}
              <button class="link" onclick={() => showInHistory(e.host, e.path)}>Show in History</button>
            </div>
          {/if}
        {/each}
      </div>
    {:else}
      <div class="hint">
        <h2>Site Map</h2>
        <p>Every host and endpoint seen so far. IDs in paths are grouped (<code>/users/{'{id}'}</code>).
          Select a node to see its endpoints; double-click to expand. <b>AI</b> marks what agents touched, ★ what was marked important.</p>
      </div>
    {/if}
{/snippet}

<style>
  /* The in-scope host: the same purple as a purple row in History (selection still wins). */
  .node.scope {
    --c: var(--c-purple);
    background: color-mix(in srgb, var(--c) 14%, transparent);
    box-shadow: inset 3px 0 var(--c);
  }
  .sitemap {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(260px, 34%) 1fr;
  }
  .tree {
    min-height: 0;
    overflow: auto;
    border-right: 1px solid var(--hairline);
    padding: 8px 6px;
  }
  .tools {
    display: flex;
    gap: 6px;
    margin: 0 4px 8px;
  }
  .scope-only {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    padding: 0 10px;
    border: 1px solid var(--hairline);
    border-radius: 999px;
    background: none;
    color: var(--text-2);
    font-size: 11.5px;
  }
  .scope-only:hover:not(:disabled) {
    color: var(--c-purple);
    border-color: color-mix(in srgb, var(--c-purple) 50%, transparent);
  }
  .scope-only.on {
    color: #fff;
    background: var(--c-purple);
    border-color: transparent;
  }
  .scope-only:disabled {
    opacity: 0.4;
  }
  .search {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 10px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: var(--field);
    color: var(--text);
    font: 12.5px var(--mono);
    outline: none;
  }
  .search:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  .node {
    position: relative;
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding-right: 8px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text);
    text-align: left;
    white-space: nowrap;
  }
  .node:hover {
    background: var(--hover);
  }
  .node.on {
    background: var(--selected);
  }
  .node.root {
    height: 30px;
    margin-top: 2px;
  }
  .guide {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px solid var(--hairline);
  }
  .elbow {
    position: absolute;
    top: 0;
    height: 50%;
    width: 9px;
    border-left: 1px solid var(--hairline);
    border-bottom: 1px solid var(--hairline);
    border-bottom-left-radius: 5px;
  }
  .node:hover .guide,
  .node:hover .elbow {
    border-color: var(--text-3);
  }
  .chev {
    position: relative; /* above the guide lines */
    width: 12px;
    display: inline-flex;
    justify-content: center;
    color: var(--text-3);
    transition: transform 140ms ease;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .chev.leaf {
    visibility: hidden;
  }
  .icon {
    display: inline-flex;
    flex: none;
    color: var(--text-3);
  }
  .icon.host {
    color: var(--text-2);
  }
  /* File types: a little color helps scanning; nothing red, so nothing looks like a finding. */
  .ft.markup {
    color: var(--c-orange);
  }
  .ft.script {
    color: var(--c-yellow);
  }
  .ft.style {
    color: var(--c-blue);
  }
  .ft.data {
    color: var(--c-green);
  }
  .ft.image {
    color: var(--ic-repeater);
  }
  .ft.font,
  .ft.text {
    color: var(--c-gray);
  }
  .ft.config {
    color: var(--ic-sitemap);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    font: 12.5px var(--mono);
  }
  .name.host {
    font-weight: 600;
  }
  .count {
    margin-left: auto;
    color: var(--text-3);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .star {
    color: var(--orange);
    font-size: 12px;
  }
  .ai {
    padding: 0 5px;
    border-radius: 6px;
    font-size: 10.5px;
    font-weight: 600;
    color: var(--accent);
    background: var(--selected);
  }
  .detail {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 14px 18px;
    border-bottom: 1px solid var(--hairline);
  }
  header h2 {
    margin: 0;
    font: 600 14px var(--mono);
  }
  .sub {
    color: var(--text-2);
    font-size: 12px;
  }
  .primary {
    margin-left: auto;
    border: 0;
    border-radius: 8px;
    padding: 5px 14px;
    background: var(--accent);
    color: #fff;
    font-weight: 500;
  }
  .table {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 6px 10px;
  }
  .tr {
    width: 100%;
    display: grid;
    grid-template-columns: 110px minmax(160px, 1fr) 80px 200px 90px;
    gap: 12px;
    align-items: center;
    min-height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text);
    text-align: left;
  }
  button.tr:hover {
    background: var(--hover);
  }
  button.tr.on {
    background: var(--selected);
  }
  .requests {
    margin: 2px 0 8px 20px;
    padding: 4px 0 4px 10px;
    border-left: 2px solid var(--selected);
    display: grid;
  }
  .req {
    display: grid;
    grid-template-columns: 50px 70px 56px minmax(120px, 1fr) 40px 64px;
    gap: 10px;
    align-items: center;
    min-height: 24px;
    padding: 0 8px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text);
    text-align: left;
    font-size: 12px;
  }
  .req:hover {
    background: var(--hover);
  }
  .req.on {
    background: var(--selected);
  }
  .meta-tags {
    grid-column: 4 / -1;
    display: flex;
    gap: 4px;
    margin-bottom: 3px;
  }
  .tag {
    padding: 0 6px;
    border-radius: 8px;
    font-size: 10.5px;
    color: var(--accent);
    background: var(--selected);
  }
  .badge {
    padding: 0 6px;
    border-radius: 8px;
    font-size: 10.5px;
    font-weight: 600;
    color: #fff;
    background: var(--sev);
  }
  .req .seq,
  .req .when,
  .req .size {
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .req .m {
    font-weight: 500;
  }
  .req .p {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font: 11.5px var(--mono);
  }
  .none {
    margin: 4px 8px;
    color: var(--text-3);
    font-size: 12px;
  }
  .link {
    justify-self: start;
    margin-top: 4px;
    padding: 2px 8px;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: 12px;
  }
  .th {
    font-size: 11.5px;
    font-weight: 500;
    color: var(--text-2);
  }
  .methods {
    font-size: 11.5px;
    font-weight: 500;
    color: var(--text-2);
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font: 12px var(--mono);
  }
  .num {
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
  }
  .classes {
    display: flex;
    gap: 4px;
  }
  .classes i {
    font-style: normal;
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
    background: var(--field);
    padding: 0 6px;
    border-radius: 6px;
  }
  .flags {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
  }
  .hint,
  .empty {
    margin: auto;
    padding: 20px;
    max-width: 460px;
    color: var(--text-2);
    text-align: center;
    line-height: 1.5;
  }
  .hint h2 {
    color: var(--text);
    font-size: 17px;
  }
</style>
