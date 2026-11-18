<script lang="ts">
  import HttpMessage from '../components/HttpMessage.svelte';
  import SideSplit from '../components/SideSplit.svelte';
  import ContextMenu from '../components/ContextMenu.svelte';
  import { getDetail } from '../lib/detail';
  import { bytes, statusClass } from '../lib/format';
  import { nav } from '../lib/nav.svelte';
  import { closeTab, openTab, rawOf, repeater, sendTab, updateTab } from '../lib/repeater.svelte';
  import type { FlowDetail, RepeaterTab, Target } from '../lib/types';
  import { highlightRequest } from '../lib/highlight';

  const tab = $derived<RepeaterTab | undefined>(
    repeater.tabs.find((t) => t.id === nav.repeaterTab) ?? repeater.tabs[0],
  );

  // Local editing state for the front tab; saved to the daemon after a pause.
  let draft = $state('');
  let colors: HTMLPreElement | undefined = $state();
  /** Keeps the last line's height when the text ends in a newline. */
  const NL = String.fromCharCode(10);
  let target = $state<Target>({ scheme: 'https', host: '', port: 443 });
  let shownId = $state<string | null>(null); // which tab the editor holds
  let viewing = $state(-1); // index into tab.sends being shown; -1 = the latest
  let result = $state.raw<(FlowDetail & { redirects?: number }) | null>(null);
  /** Right-click on the response: the same request menu as everywhere else. */
  let menu = $state<{ x: number; y: number } | null>(null);
  /** Follow redirects when sending; on by default, remembered in this browser. */
  let follow = $state(localStorage.getItem('shiftwire.followRedirects') !== 'false');
  $effect(() => localStorage.setItem('shiftwire.followRedirects', String(follow)));
  let sending = $state(false);
  let problem = $state<string | null>(null);
  let renaming = $state(false);
  let saveTimer: ReturnType<typeof setTimeout> | undefined;

  // Switching tabs (or a tab arriving): load it into the editor.
  $effect(() => {
    if (!tab || tab.id === shownId) return;
    shownId = tab.id;
    draft = tab.raw;
    target = { ...tab.target };
    viewing = -1;
    problem = null;
    result = null;
    if (tab.sends.length) void show(tab.sends.length - 1, false, true);
  });

  function scheduleSave() {
    const id = shownId;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => id && updateTab(id, { raw: draft, target: { ...target } }).catch(report), 400);
  }

  function setScheme(scheme: 'http' | 'https') {
    const defaults = { http: 80, https: 443 };
    if (target.port === defaults[target.scheme]) target.port = defaults[scheme];
    target.scheme = scheme;
    scheduleSave();
  }

  async function send() {
    if (!tab || sending) return;
    clearTimeout(saveTimer);
    sending = true;
    problem = null;
    try {
      result = await sendTab(tab.id, draft, { ...target, port: Number(target.port) }, follow);
      viewing = -1; // the newest send; the pushed tab list may arrive before or after this
    } catch (e) {
      report(e);
    } finally {
      sending = false;
    }
  }

  /** Show an earlier send: its response, and its request in the editor. */
  async function show(i: number, loadRequest = true, latest = false) {
    if (!tab) return;
    const id = tab.sends[i];
    if (!id) return;
    viewing = latest || i === tab.sends.length - 1 ? -1 : i;
    result = await getDetail(id);
    if (loadRequest) {
      draft = await rawOf(id);
      scheduleSave();
    }
  }

  function report(e: unknown) {
    problem = e instanceof Error ? e.message : String(e);
  }

  function rename(e: Event) {
    const name = (e.target as HTMLInputElement).value;
    renaming = false;
    if (tab && name.trim()) void updateTab(tab.id, { name });
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void send();
    }
  }

  const total = $derived(tab?.sends.length ?? 0);
  const position = $derived(viewing < 0 ? total : viewing + 1);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<section class="repeater" onkeydown={onKey}>
  <nav class="tabs" aria-label="Repeater tabs">
    {#each repeater.tabs as t (t.id)}
      <div class="tab" class:on={t.id === tab?.id}>
        {#if renaming && t.id === tab?.id}
          <!-- svelte-ignore a11y_autofocus -->
          <input class="rename" value={t.name} autofocus onblur={rename} onkeydown={(e) => e.key === 'Enter' && rename(e)} />
        {:else}
          <button class="name" title="{t.name} (double-click to rename)" onclick={() => (nav.repeaterTab = t.id)} ondblclick={() => (renaming = true)}>
            {t.name}
          </button>
        {/if}
        <button class="close" title="Close tab" aria-label="Close tab" onclick={() => closeTab(t.id)}>×</button>
      </div>
    {/each}
    <button class="add" title="New blank request" aria-label="New request" onclick={() => openTab()}>+</button>
  </nav>

  {#if !tab}
    <div class="empty">
      <h2>No requests yet</h2>
      <p>In History, right-click a request and choose <b>Send to Repeater</b> (or select it and press <kbd>R</kbd>).</p>
      <button class="primary" onclick={() => openTab()}>New blank request</button>
    </div>
  {:else}
    <div class="bar">
      <div class="segmented" role="group" aria-label="Scheme">
        <button class:on={target.scheme === 'http'} onclick={() => setScheme('http')}>http</button>
        <button class:on={target.scheme === 'https'} onclick={() => setScheme('https')}>https</button>
      </div>
      <input class="host" bind:value={target.host} oninput={scheduleSave} spellcheck="false" aria-label="Host" placeholder="host" />
      <input class="port" type="number" min="1" max="65535" bind:value={target.port} oninput={scheduleSave} aria-label="Port" />
      <label class="follow" title="Follow redirects (up to 10); each step is also recorded in History">
        <input type="checkbox" bind:checked={follow} /> Follow redirects
      </label>
      <div class="history" title="Earlier sends from this tab">
        <button disabled={position <= 1} onclick={() => show(position - 2)} aria-label="Previous send">‹</button>
        <span>{total ? `${position} / ${total}` : '–'}</span>
        <button disabled={position >= total} onclick={() => show(position)} aria-label="Next send">›</button>
      </div>
      <button class="primary send" onclick={send} disabled={sending} title="Send (Ctrl+Enter)">
        {sending ? 'Sending…' : 'Send'}
      </button>
    </div>
    {#if problem}<p class="problem">{problem}</p>{/if}

    <div class="panes">
      <SideSplit name="repeater">
      {#snippet left()}
      <div class="editor">
        <header><h3>Request</h3><span class="hint">Ctrl+Enter sends · Content-Length is updated for you</span></header>
        <div class="code syntax">
          <!-- Colors drawn behind a transparent textarea: plain editing, highlighted text. -->
          <pre bind:this={colors} aria-hidden="true">{@html highlightRequest(draft) + NL}</pre>
          <textarea
            bind:value={draft}
            oninput={scheduleSave}
            onscroll={(e) => colors && ((colors.scrollTop = e.currentTarget.scrollTop), (colors.scrollLeft = e.currentTarget.scrollLeft))}
            spellcheck="false"
            aria-label="Raw request"
          ></textarea>
        </div>
      </div>
      {/snippet}
      {#snippet right()}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="response"
        oncontextmenu={(e) => result && !window.getSelection()?.toString() && (e.preventDefault(), (menu = { x: e.clientX, y: e.clientY }))}
      >
        {#if result}
          <div class="meta">
            <span class="seq" title="This send's number in History">#{result.seq}</span>
            <span class="status {statusClass(result.status)}">{result.status ?? 'ERR'}</span>
            · {bytes(result.size)} · {result.duration_ms} ms
            {#if result.redirects}· after {result.redirects} redirect{result.redirects === 1 ? '' : 's'}: <span class="final">{result.host}{result.path}</span>{/if}
            {#if result.error}<span class="err">{result.error}</span>{/if}
          </div>
          <HttpMessage title="Response" pane="response" message={result.response} />
        {:else}
          <p class="waiting">{sending ? 'Waiting for the response…' : 'Send the request to see the response here.'}</p>
        {/if}
      </div>
      {/snippet}
      </SideSplit>
    </div>
  {/if}
</section>

{#if menu && result}
  <ContextMenu x={menu.x} y={menu.y} flows={[result]} onclose={() => (menu = null)} />
{/if}

<style>
  .seq {
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
    margin-right: 4px;
  }
  .follow {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--text-2);
    white-space: nowrap;
    user-select: none;
  }
  .final {
    font-family: var(--mono);
    color: var(--text);
  }
  .repeater {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 12px;
    border-bottom: 1px solid var(--hairline);
    overflow-x: auto;
  }
  .tab {
    display: flex;
    align-items: center;
    max-width: 220px;
    border-radius: 7px;
  }
  .tab:hover {
    background: var(--hover);
  }
  .tab.on {
    background: var(--selected);
  }
  .tab .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border: 0;
    background: none;
    padding: 4px 4px 4px 10px;
    color: var(--text);
    font: 12px var(--mono);
  }
  .tab.on .name {
    color: var(--accent);
  }
  .tab .close {
    border: 0;
    background: none;
    color: var(--text-3);
    padding: 2px 8px 2px 4px;
    visibility: hidden;
  }
  .tab:hover .close,
  .tab.on .close {
    visibility: visible;
  }
  .rename {
    width: 180px;
    font: 12px var(--mono);
    border: 1px solid var(--accent);
    border-radius: 6px;
    padding: 3px 6px;
    background: var(--surface);
    color: var(--text);
    outline: none;
  }
  .add {
    border: 0;
    background: none;
    color: var(--text-2);
    font-size: 16px;
    width: 26px;
    height: 26px;
    border-radius: 7px;
  }
  .add:hover {
    background: var(--hover);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--hairline);
  }
  .segmented {
    display: flex;
    padding: 2px;
    border-radius: 7px;
    background: var(--field);
  }
  .segmented button {
    border: 0;
    background: none;
    color: var(--text-2);
    font-size: 12px;
    padding: 3px 10px;
    border-radius: 5px;
  }
  .segmented button.on {
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
  }
  .bar input {
    height: 28px;
    padding: 0 9px;
    border: 1px solid transparent;
    border-radius: 7px;
    background: var(--field);
    color: var(--text);
    font: 12.5px var(--mono);
    outline: none;
  }
  .bar input:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  .host {
    flex: 1;
    min-width: 120px;
  }
  .port {
    width: 76px;
  }
  .history {
    display: flex;
    align-items: center;
    gap: 2px;
    font-size: 12px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  .history span {
    min-width: 46px;
    text-align: center;
  }
  .history button {
    border: 0;
    background: none;
    color: var(--text);
    font-size: 16px;
    width: 24px;
    height: 26px;
    border-radius: 6px;
  }
  .history button:hover:not(:disabled) {
    background: var(--hover);
  }
  .primary {
    border: 0;
    border-radius: 7px;
    padding: 5px 16px;
    background: var(--accent);
    color: #fff;
    font-weight: 500;
  }
  .primary:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .send {
    min-width: 84px;
  }
  .problem {
    margin: 0;
    padding: 7px 14px;
    color: var(--red);
    background: color-mix(in srgb, var(--red) 8%, transparent);
    font-size: 12.5px;
  }
  .panes {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .editor,
  .response {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .editor header {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 12px;
    border-bottom: 1px solid var(--hairline);
  }
  h3 {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .hint {
    margin-left: auto;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .code {
    flex: 1;
    min-height: 0;
    position: relative;
    background: var(--surface);
  }
  .code pre,
  .code textarea {
    position: absolute;
    inset: 0;
    margin: 0;
    padding: 10px 12px;
    border: 0;
    overflow-y: scroll; /* both always reserve the scrollbar, so lines wrap at the same width */
    overflow-x: hidden;
    font: 12px/1.5 var(--mono);
    tab-size: 4;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .code pre {
    color: var(--text);
    pointer-events: none;
  }
  .code textarea {
    resize: none;
    outline: none;
    background: transparent;
    color: transparent;
    caret-color: var(--text);
  }
  .code textarea::selection {
    color: transparent;
    background: color-mix(in srgb, var(--accent) 30%, transparent);
  }
  .meta {
    padding: 6px 12px;
    border-bottom: 1px solid var(--hairline);
    font-size: 12px;
    color: var(--text-2);
  }
  .meta .err {
    margin-left: 6px;
    color: var(--red);
  }
  .waiting {
    margin: auto;
    color: var(--text-3);
  }
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    text-align: center;
  }
  .empty h2 {
    margin: 0;
    font-size: 17px;
  }
  .empty p {
    margin: 0 0 6px;
    max-width: 420px;
    color: var(--text-2);
    line-height: 1.5;
  }
  kbd {
    font: 11.5px var(--mono);
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--field);
  }
</style>
