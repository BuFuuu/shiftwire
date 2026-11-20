<script lang="ts">
  import HttpMessage from '../components/HttpMessage.svelte';
  import type { MenuItem } from '../components/TextMenu.svelte';
  import { copyText } from '../lib/text';
  import { toCurl } from '../lib/curl';
  import SideSplit from '../components/SideSplit.svelte';
  import ContextMenu from '../components/ContextMenu.svelte';
  import { openTab } from '../lib/repeater.svelte';
  import { getDetail } from '../lib/detail';
  import type { FlowDetail } from '../lib/types';

  let { id, onclose }: { id: string; onclose: () => void } = $props();

  let detail = $state.raw<FlowDetail | null>(null);
  let failed = $state(false);

  $effect(() => {
    const wanted = id;
    failed = false;
    getDetail(wanted).then((d) => {
      if (wanted !== id) return; // the user moved on meanwhile
      detail = d;
      failed = d === null;
    });
  });

  // Right-click anywhere in the details: the same request menu as everywhere else.
  let menu = $state<{ x: number; y: number } | null>(null);
  function onContextMenu(e: MouseEvent) {
    if (window.getSelection()?.toString()) return; // keep the browser menu for copying a selection
    e.preventDefault();
    menu = { x: e.clientX, y: e.clientY };
  }

  // Esc closes the details in every view (not while typing or with a menu open).
  function onKey(e: KeyboardEvent) {
    const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
    if (e.key === 'Escape' && !typing && !menu && !document.querySelector('[role=menu]')) onclose();
  }

  /** Extra right-click items in the request and response panes. */
  const extras = (pane: 'request' | 'response') => (x: number, y: number): MenuItem[] => [
    ...(pane === 'request' && detail
      ? [
          { label: 'Copy URL', action: () => copyText(url) },
          { label: 'Copy as curl', action: () => copyText(toCurl(detail!)) },
          { label: 'Send to Repeater', disabled: detail.method === 'CONNECT', action: () => openTab(detail!.id) },
        ]
      : []),
    { label: 'Color, tags, mark, filter…', action: () => (menu = { x, y }) },
  ];

  const url = $derived(
    detail ? `${detail.scheme}://${detail.host}${[80, 443].includes(detail.port) ? '' : `:${detail.port}`}${detail.path}` : '',
  );
</script>

<svelte:window onkeydown={onKey} />

<div class="detail">
  {#if detail && detail.id === id}
    {#if detail.error}<p class="error">{detail.error}</p>{/if}
    {#if detail.mark}
      <p class="meta-line"><span class="mark sev-{detail.mark}">★ {detail.mark}</span> {detail.mark_reason}</p>
    {/if}
    {#if detail.tags.length || detail.note}
      <p class="meta-line">
        {#each detail.tags as t (t)}<span class="tag">{t}</span>{/each}
        {#if detail.note}<span class="note">✎ {detail.note}</span>{/if}
      </p>
    {/if}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="panes" oncontextmenu={onContextMenu}>
      <SideSplit name="detail">
        {#snippet left()}<HttpMessage title="Request" pane="request" message={detail!.request} extra={extras('request')} />{/snippet}
        {#snippet right()}<HttpMessage title="Response" pane="response" message={detail!.response} extra={extras('response')} {onclose} />{/snippet}
      </SideSplit>
    </div>
    {#if menu}
      <ContextMenu x={menu.x} y={menu.y} flows={[detail]} onclose={() => (menu = null)} />
    {/if}
  {:else if failed}
    <p class="loading">Could not load this request.</p>
  {:else}
    <p class="loading">Loading…</p>
  {/if}
</div>

<style>
  .detail {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
  }
  .error {
    margin: 0;
    padding: 7px 16px;
    color: var(--red);
    background: color-mix(in srgb, var(--red) 8%, transparent);
    font-size: 12.5px;
  }
  .meta-line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 0;
    padding: 6px 16px;
    border-bottom: 1px solid var(--hairline);
    font-size: 12.5px;
  }
  .tag {
    padding: 0 7px;
    border-radius: 8px;
    font-size: 11.5px;
    color: var(--accent);
    background: var(--selected);
  }
  .mark {
    padding: 0 7px;
    border-radius: 8px;
    font-size: 11.5px;
    font-weight: 600;
    color: #fff;
    background: var(--sev);
  }
  .note {
    color: var(--text-2);
    white-space: pre-wrap;
  }
  .panes {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .loading {
    margin: auto;
    color: var(--text-3);
  }
</style>
