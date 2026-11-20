<script lang="ts">
  // The MCP log: every MCP request the agents sent and exactly what they got back,
  // newest first. A click shows the full request and response below.
  import SplitPane from '../components/SplitPane.svelte';
  import VirtualList from '../components/VirtualList.svelte';
  import { mcpLog } from '../lib/live.svelte';
  import { time } from '../lib/format';
  import type { McpCall } from '../lib/types';
  import TextMenu, { type MenuItem } from '../components/TextMenu.svelte';
  import { base64Decode, copyText, selectedText, urlDecode } from '../lib/text';

  let textMenu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);
  function paneMenu(e: MouseEvent, label: string, text: string) {
    e.preventDefault();
    const sel = selectedText();
    textMenu = {
      x: e.clientX,
      y: e.clientY,
      items: [
        { label: 'Copy selection', hint: 'Ctrl+C', disabled: !sel, action: () => copyText(sel) },
        { label: `Copy ${label}`, action: () => copyText(text) },
        null,
        { label: 'Copy selection URL-decoded', disabled: !sel, action: () => copyText(urlDecode(sel)) },
        { label: 'Copy selection base64-decoded', disabled: !sel, action: () => copyText(base64Decode(sel)) },
      ],
    };
  }

  const ROW = 26;
  let agent = $state<string | null>(null);
  let search = $state('');
  let errorsOnly = $state(false);
  let selected = $state<string | null>(null);
  let full = $state<McpCall | null>(null);

  const agents = $derived([...new Set(mcpLog.calls.map((c) => c.agent))]);
  const name = (c: McpCall) => c.tool ?? c.method;
  const oneLine = (text: string) => text.replace(/\s*\n\s*/g, ' ');

  const view = $derived.by(() => {
    const q = search.trim().toLowerCase();
    return mcpLog.calls
      .filter((c) => (!agent || c.agent === agent) && (!errorsOnly || c.is_error))
      .filter((c) => !q || `${c.agent} ${name(c)} ${c.request} ${c.response}`.toLowerCase().includes(q))
      .toReversed();
  });

  // The list holds previews; the detail pane loads the whole call.
  $effect(() => {
    const id = selected;
    full = null;
    if (!id) return;
    void fetch(`/api/mcp-calls/${id}`)
      .then((r) => (r.ok ? r.json() : null))
      .then((call) => {
        if (selected === id) full = call;
      });
  });

  function onKey(e: KeyboardEvent) {
    const i = view.findIndex((c) => c.id === selected);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const next = view[Math.min(view.length - 1, Math.max(0, i + (e.key === 'ArrowDown' ? 1 : -1)))];
      if (next) selected = next.id;
    } else if (e.key === 'Escape') {
      selected = null;
    }
  }

  /** Double-click marks a call purple, like a request in History. */
  function toggleColor(c: McpCall) {
    void fetch('/api/mcp-calls/color', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ids: [c.id], color: c.color === 'purple' ? null : 'purple' }),
    });
  }

  const size = (n: number) => (n < 1000 ? `${n}` : `${(n / 1000).toFixed(1)}k`);
</script>

{#snippet list()}
  <div class="bar">
    <div class="chips">
      <button class:on={!agent} onclick={() => (agent = null)}>All agents</button>
      {#each agents as a (a)}
        <button class:on={agent === a} onclick={() => (agent = agent === a ? null : a)}>{a}</button>
      {/each}
      <button class:on={errorsOnly} onclick={() => (errorsOnly = !errorsOnly)}>Errors only</button>
    </div>
    <input class="search" bind:value={search} placeholder="Search tools, arguments and replies" spellcheck="false" />
    <span class="count">{view.length.toLocaleString()} call{view.length === 1 ? '' : 's'}</span>
  </div>
  <div class="head">
    <span>Time</span><span>Agent</span><span>Call</span><span>Arguments</span><span>Reply</span><span>Size</span><span>ms</span>
  </div>
  {#if view.length === 0}
    <div class="empty">
      {#if mcpLog.calls.length === 0}
        <h2>No MCP calls yet</h2>
        <p>When an agent uses shiftWire's MCP, every request and the exact reply it got appear here.</p>
      {:else}
        <p>No call matches.</p>
      {/if}
    </div>
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="rows" tabindex="0" role="grid" aria-label="MCP calls" onkeydown={onKey}>
      <VirtualList items={view} key={(c) => c.id} rowHeight={ROW}>
        {#snippet row(c: McpCall, i: number)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div class="row" class:odd={i % 2 === 1} class:picked={selected === c.id} class:failed={c.is_error}
            class:colored={c.color} style:--c={c.color ? `var(--c-${c.color})` : null}
            role="row" tabindex="-1" onclick={() => (selected = c.id)} ondblclick={() => toggleColor(c)}
            title="Double-click to mark it purple">
            <span class="num">{time(c.ts)}</span>
            <span><span class="agent">{c.agent}</span></span>
            <span class="call">{name(c)}</span>
            <span class="clip mono" title={c.request}>{oneLine(c.request)}</span>
            <span class="clip reply" title={c.response}>{#if c.is_error}<b>✕</b> {/if}{oneLine(c.response)}</span>
            <span class="num">{size(c.response_size)}</span>
            <span class="num">{c.duration_ms}</span>
          </div>
        {/snippet}
      </VirtualList>
    </div>
  {/if}
{/snippet}

{#snippet detail()}
  <div class="detail">
    {#if full}
      <header>
        <span class="call">{name(full)}</span>
        <span class="muted">{full.tool ? full.method : ''}</span>
        <span class="agent">{full.agent}</span>
        <span class="muted">{new Date(full.ts).toLocaleString()} · {full.duration_ms} ms</span>
        {#if full.is_error}<span class="bad">error</span>{/if}
        <button class="close" onclick={() => (selected = null)} title="Close (Esc)">✕</button>
      </header>
      <div class="panes">
        <section>
          <h3>{full.tool ? 'Arguments' : 'Params'} <span class="muted">{size(full.request_size)} chars</span></h3>
          <pre oncontextmenu={(e) => paneMenu(e, full!.tool ? 'arguments' : 'params', full!.request)}>{full.request}</pre>
        </section>
        <section>
          <h3>Reply the agent got <span class="muted">{size(full.response_size)} chars</span></h3>
          <pre class:bad={full.is_error} oncontextmenu={(e) => paneMenu(e, 'reply', full!.response)}>{full.response}</pre>
        </section>
      </div>
    {:else}
      <p class="muted loading">Loading…</p>
    {/if}
  </div>
{/snippet}

{#if textMenu}
  <TextMenu x={textMenu.x} y={textMenu.y} items={textMenu.items} onclose={() => (textMenu = null)} />
{/if}

<section class="mcplog">
  <SplitPane top={list} bottom={selected ? detail : undefined} />
</section>

<style>
  .mcplog {
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
  .chips button {
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
  .search {
    flex: 1;
    min-width: 160px;
    height: 26px;
    padding: 0 9px;
    border: 1px solid var(--hairline);
    border-radius: 7px;
    background: var(--field);
    color: var(--text);
    font-size: 12px;
    outline: none;
  }
  .count {
    font-size: 11.5px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
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
    grid-template-columns: 64px 100px 120px minmax(120px, 1fr) minmax(160px, 1.6fr) 48px 44px;
    gap: 12px;
    align-items: center;
    padding: 0 16px;
    white-space: nowrap;
  }
  .head {
    height: 30px;
    flex: none;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-2);
    border-bottom: 1px solid var(--hairline);
    scrollbar-gutter: stable;
  }
  .row {
    height: 26px;
    font-size: 12.5px;
    cursor: default;
  }
  .row.odd {
    background: var(--stripe);
  }
  .row:hover {
    background: var(--hover);
  }
  .row.colored {
    background: color-mix(in srgb, var(--c) 14%, transparent); /* the operator's color stays visible on striped and hovered rows */
  }
  .row.picked {
    background: var(--accent);
    color: #fff;
  }
  .row.picked .num,
  .row.picked .mono,
  .row.picked .reply,
  .row.picked .call {
    color: inherit;
  }
  .row.picked .agent {
    color: #fff;
    background: rgba(255, 255, 255, 0.22);
  }
  .num {
    font-variant-numeric: tabular-nums;
    color: var(--text-2);
  }
  .clip {
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .mono,
  .call {
    font: 12px var(--mono);
  }
  .call {
    font-weight: 600;
    color: var(--ic-timeline);
  }
  .reply {
    color: var(--text-2);
  }
  .failed .reply,
  .bad {
    color: var(--red);
  }
  .agent {
    display: inline-block;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    padding: 0 7px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
    font-size: 11.5px;
  }
  .empty {
    padding: 48px 24px;
    text-align: center;
    color: var(--text-2);
  }
  .empty h2 {
    margin: 0 0 6px;
    font-size: 15px;
    color: var(--text);
  }
  .detail {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--hairline);
  }
  .detail header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--hairline);
    font-size: 12.5px;
  }
  .muted {
    color: var(--text-3);
    font-weight: 400;
  }
  .loading {
    padding: 12px 16px;
  }
  .close {
    margin-left: auto;
    border: 0;
    background: none;
    color: var(--text-2);
  }
  .panes {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 1fr 1fr;
  }
  .panes section {
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .panes section + section {
    border-left: 1px solid var(--hairline);
  }
  h3 {
    margin: 0;
    padding: 8px 16px;
    font-size: 11px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-2);
  }
  pre {
    flex: 1;
    min-height: 0;
    overflow: auto;
    margin: 0;
    padding: 4px 16px 16px;
    font: 12px/1.5 var(--mono);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
