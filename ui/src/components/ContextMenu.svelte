<script lang="ts">
  import { untrack } from 'svelte';
  // The right-click menu for requests, the same in every view (History, Timeline,
  // Site Map, request details): color, tags, note, mark, copy, filter, Repeater.
  import { colorFlows, noteFlows, tagFlows } from '../lib/api';
  import { toCurl, flowUrl } from '../lib/curl';
  import { getDetail } from '../lib/detail';
  import { addPin, jumpTo } from '../lib/live.svelte';
  import { BURST_GAPS, DEFAULT_GAP, showBurst } from '../lib/around';
  import { nav } from '../lib/nav.svelte';
  import { openTab } from '../lib/repeater.svelte';
  import { COLORS, SEVERITIES, type FlowSummary } from '../lib/types';

  let {
    x,
    y,
    flows,
    focus = null,
    onclose,
  }: { x: number; y: number; flows: FlowSummary[]; focus?: 'tag' | 'note' | null; onclose: () => void } = $props();

  const ids = $derived(flows.map((f) => f.id));
  const first = $derived(flows[0]);
  const tags = $derived([...new Set(flows.flatMap((f) => f.tags))].sort());

  let menu: HTMLDivElement | undefined = $state();
  let tagInput: HTMLInputElement | undefined = $state();
  let noteInput: HTMLTextAreaElement | undefined = $state();
  let newTag = $state('');
  // Start from the values at opening time; the menu is recreated for each opening.
  let note = $state(untrack(() => flows[0]?.note ?? ''));
  let pos = $state(untrack(() => ({ left: x, top: y })));

  // Keep the menu on screen, then focus what the shortcut asked for.
  $effect(() => {
    if (!menu) return;
    const r = menu.getBoundingClientRect();
    pos = { left: Math.min(x, innerWidth - r.width - 8), top: Math.min(y, innerHeight - r.height - 8) };
    (focus === 'tag' ? tagInput : focus === 'note' ? noteInput : menu)?.focus();
  });

  function addTag(e: KeyboardEvent) {
    if (e.key !== 'Enter') return;
    const tag = newTag.trim().replaceAll(/\s+/g, '-');
    if (tag) void tagFlows(ids, [tag]);
    onclose();
  }

  let markReason = $state('');
  async function mark(severity: string) {
    const reason = markReason.trim() || 'marked by the operator';
    await Promise.all(
      ids.map((id) =>
        fetch('/api/flows/mark', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ id, severity, reason }),
        }),
      ),
    );
    onclose();
  }

  function saveNote() {
    void noteFlows(ids, note);
    onclose();
  }

  async function copy(text: string | Promise<string>) {
    await navigator.clipboard.writeText(await text);
    onclose();
  }

  async function curl(): Promise<string> {
    const detail = await getDetail(first.id);
    return detail ? toCurl(detail) : '';
  }

  /** Lasting chips (removable with ×) instead of editing the main filter. */
  /** Lasting chips; from another view, also go to History to see the result. */
  function pin(mode: 'hide' | 'show', text: string) {
    addPin(mode, text);
    nav.view = 'history';
    onclose();
  }

  function showInHistory() {
    const text = flows.map((f) => `id:${f.seq}`).join(' OR ');
    jumpTo(flows.length === 1 ? text : `(${text})`, flows.length === 1 ? `#${first.seq}` : `${flows.length} requests`);
    onclose();
  }


  function onWindowPointer(e: PointerEvent) {
    if (menu && !menu.contains(e.target as Node)) onclose();
  }
</script>

<svelte:window onpointerdown={onWindowPointer} onblur={onclose} />

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="menu"
  bind:this={menu}
  role="menu"
  tabindex="-1"
  style:left="{pos.left}px"
  style:top="{pos.top}px"
  onkeydown={(e) => e.key === 'Escape' && onclose()}
>
  <div class="label">{flows.length === 1 ? `${first.method} ${first.host}` : `${flows.length} requests`}</div>

  <div class="swatches">
    {#each COLORS as c, i (c)}
      <button class="swatch" style:--c="var(--c-{c})" title="{c} ({i + 1})" aria-label={c} onclick={() => (colorFlows(ids, c), onclose())}></button>
    {/each}
    <button class="swatch none" title="No color (0)" aria-label="no color" onclick={() => (colorFlows(ids, null), onclose())}></button>
  </div>

  <div class="sep"></div>
  <input bind:this={tagInput} bind:value={newTag} onkeydown={addTag} placeholder="Add tag… (t)" spellcheck="false" />
  {#if tags.length}
    <div class="chips">
      {#each tags as t (t)}
        <button class="chip" title="Remove tag" onclick={() => (tagFlows(ids, [], [t]), onclose())}>{t} <span>×</span></button>
      {/each}
    </div>
  {/if}
  <textarea
    bind:this={noteInput}
    bind:value={note}
    rows="2"
    placeholder="Note… (n) · Ctrl+Enter saves"
    onkeydown={(e) => e.key === 'Enter' && (e.metaKey || e.ctrlKey) && saveNote()}
  ></textarea>
  {#if note !== (first?.note ?? '')}<button class="item save" onclick={saveNote}>Save note</button>{/if}

  <div class="sep"></div>
  <input bind:value={markReason} placeholder="Why is it important? (optional)" spellcheck="false" />
  <div class="sevs">
    <span>Mark</span>
    {#each SEVERITIES.slice(1) as sev (sev)}
      <button class="sev sev-{sev}" onclick={() => mark(sev)}>{sev}</button>
    {/each}
  </div>

  <div class="sep"></div>
  <button class="item" onclick={() => copy(flowUrl(first))}>Copy URL</button>
  <button class="item" onclick={() => copy(curl())}>Copy as curl</button>
  <div class="sep"></div>
  <button class="item" onclick={() => pin('show', `host:=${first.host}`)}>Only {first.host}</button>
  <button class="item" onclick={() => pin('hide', `host:=${first.host}`)}>Hide {first.host}</button>
  <div class="burst" title="The run of requests by {first.actor} this one belongs to: no pause between two of them longer than the gap. Roughly what one click or page load sent.">
    <button class="item" onclick={() => (void showBurst(first, DEFAULT_GAP), onclose())}>Show burst</button>
    <div class="gaps" role="group" aria-label="Longest pause inside the burst">
      {#each BURST_GAPS as s (s)}
        <button class:default={s === DEFAULT_GAP} title="Pauses up to {s} s" onclick={() => (void showBurst(first, s), onclose())}>{s}s</button>
      {/each}
    </div>
  </div>
  <div class="sep"></div>
  <button class="item" disabled={first.method === 'CONNECT'} onclick={() => (openTab(first.id), onclose())}>
    Send to Repeater{#if nav.view === 'history'} <kbd>R</kbd>{/if}
  </button>
  {#if nav.view !== 'history'}
    <button class="item" onclick={showInHistory}>Show in History</button>
  {/if}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 50;
    width: 240px;
    padding: 6px;
    border-radius: 10px;
    border: 1px solid var(--hairline);
    background: color-mix(in srgb, var(--surface) 88%, transparent);
    backdrop-filter: saturate(180%) blur(24px);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.18);
    outline: none;
    animation: appear 90ms ease-out;
  }
  @keyframes appear {
    from {
      opacity: 0;
      transform: scale(0.98);
    }
  }
  .label {
    padding: 4px 8px 6px;
    font-size: 11.5px;
    color: var(--text-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .swatches {
    display: flex;
    gap: 6px;
    padding: 2px 8px 6px;
  }
  .swatch {
    width: 18px;
    height: 18px;
    padding: 0;
    border-radius: 50%;
    border: 0;
    background: var(--c);
    cursor: pointer;
  }
  .swatch:hover {
    transform: scale(1.15);
  }
  .swatch.none {
    background: none;
    box-shadow: inset 0 0 0 1.5px var(--text-3);
  }
  .sep {
    height: 1px;
    margin: 5px 4px;
    background: var(--hairline);
  }
  input,
  textarea {
    width: 100%;
    margin: 2px 0;
    padding: 5px 8px;
    border: 1px solid var(--hairline);
    border-radius: 6px;
    background: var(--field);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
    resize: vertical;
    outline: none;
  }
  input:focus,
  textarea:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 4px 2px;
  }
  .chip {
    border: 0;
    border-radius: 9px;
    padding: 1px 8px;
    font-size: 11.5px;
    background: var(--selected);
    color: var(--accent);
  }
  .chip span {
    opacity: 0.6;
  }
  .sevs {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 2px;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .sevs span {
    margin-right: 2px;
  }
  .sev {
    border: 0;
    border-radius: 8px;
    padding: 1px 7px;
    font-size: 11px;
    color: #fff;
    background: var(--sev);
  }
  .sev:hover {
    filter: brightness(1.1);
  }
  .burst {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .burst .item {
    flex: 1;
    width: auto;
  }
  .gaps {
    display: flex;
    flex: none;
    padding: 2px;
    border-radius: 7px;
    background: var(--field);
  }
  .gaps button {
    min-width: 26px;
    padding: 2px 5px;
    border: 0;
    border-radius: 5px;
    background: none;
    color: var(--text-2);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .gaps button.default {
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 0 0 1px var(--hairline);
  }
  .gaps button:hover {
    background: var(--accent);
    color: #fff;
  }
  .item {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 5px 8px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text);
    text-align: left;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .item:hover:not(:disabled) {
    background: var(--accent);
    color: #fff;
  }
  .item.save {
    color: var(--accent);
  }
  kbd {
    margin-left: auto;
    font: 11px var(--font);
    color: var(--text-3);
  }
</style>
