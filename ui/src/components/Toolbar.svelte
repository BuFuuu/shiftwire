<script lang="ts">
  import Icon from './Icon.svelte';
  import { history, live, setFilter } from '../lib/live.svelte';
  import { setAgentMode } from '../lib/api';
  import { nav, setAiTab } from '../lib/nav.svelte';
  import type { AgentMode } from '../lib/types';

  const modes: { id: AgentMode; label: string; help: string }[] = [
    { id: 'act', label: 'AI sends', help: 'AI agents send requests themselves, within the scope (Settings).' },
    { id: 'advise', label: 'AI advises', help: 'AI agents read and analyze, and prepare requests in the Repeater. You review and press Send.' },
    { id: 'off', label: 'AI off', help: 'Emergency stop: every request from an AI agent is refused (and shown in red).' },
  ];

  let { title, filterable = false }: { title: string; filterable?: boolean } = $props();

  let input: HTMLInputElement | undefined = $state();
  let text = $state(live.filter);
  let timer: ReturnType<typeof setTimeout> | undefined;

  // Apply while typing, after a short pause; Enter applies at once.
  function onInput() {
    clearTimeout(timer);
    timer = setTimeout(() => setFilter(text), 60); // results are paged, so this stays fast
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      clearTimeout(timer);
      setFilter(text);
    } else if (e.key === 'Escape') {
      text = '';
      setFilter('');
    } else if (e.key === 'ArrowDown') {
      // Jump from the filter into the list.
      e.preventDefault();
      (document.querySelector('.rows') as HTMLElement | null)?.focus();
    }
  }

  /** `/` anywhere focuses the filter (unless typing somewhere else). */
  function onWindowKey(e: KeyboardEvent) {
    const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
    if (filterable && e.key === '/' && !typing) {
      e.preventDefault();
      input?.focus();
      input?.select();
    }
  }

  // Opening History puts the cursor in the filter, ready to type.
  $effect(() => {
    if (filterable) input?.focus();
  });

  // Other parts of the UI (e.g. "Only this host") change the filter too.
  $effect(() => {
    if (document.activeElement !== input) text = live.filter;
  });
</script>

<svelte:window onkeydown={onWindowKey} />

<header class="toolbar">
  <h1>{title}</h1>
  {#if nav.view === 'timeline'}
    <div class="tabs" role="tablist" aria-label="AI Timeline view">
      <button role="tab" aria-selected={nav.aiTab === 'plans'} class:on={nav.aiTab === 'plans'} onclick={() => setAiTab('plans')}
        title="What the agents did, grouped by their plans (log_intent)">Plans</button>
      <button role="tab" aria-selected={nav.aiTab === 'mcp'} class:on={nav.aiTab === 'mcp'} onclick={() => setAiTab('mcp')}
        title="Every MCP request the agents sent and exactly what they got back">MCP log</button>
    </div>
  {/if}
  {#if filterable}
    <label class="filter" class:error={live.filterError} title={live.filterError ?? 'Filter (press /)'}>
      <Icon name="search" size={14} />
      <input
        bind:this={input}
        bind:value={text}
        oninput={onInput}
        onkeydown={onKey}
        spellcheck="false"
        placeholder="Filter   host:shop.test  status:>=400  actor:claude  tag:idor"
      />
      {#if live.filterError}
        <span class="problem">{live.filterError}</span>
      {:else if live.filter || live.pins.some((p) => p.on)}
        <span class="count" title="Requests matching the filter">{history.total.toLocaleString()} match{history.total === 1 ? '' : 'es'}</span>
      {/if}
    </label>
  {/if}
  <div class="modes" role="radiogroup" aria-label="What AI agents may do">
    {#each modes as m (m.id)}
      <button role="radio" aria-checked={live.agentMode === m.id} class="{m.id}" class:on={live.agentMode === m.id}
        title={m.help} disabled={!live.connected} onclick={() => setAgentMode(m.id)}>{m.label}</button>
    {/each}
  </div>
</header>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 14px;
    height: 52px;
    padding: 0 16px;
    border-bottom: 1px solid var(--hairline);
  }
  h1 {
    font-size: 15px;
    font-weight: 600;
    margin: 0;
    min-width: 110px;
  }
  .filter {
    flex: 1;
    max-width: 620px;
    display: flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 10px;
    border-radius: 8px;
    background: var(--field);
    color: var(--text-3);
    border: 1px solid transparent;
  }
  .filter:focus-within {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  .filter.error {
    border-color: color-mix(in srgb, var(--red) 60%, transparent);
  }
  .filter input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: 0;
    background: none;
    color: var(--text);
    font: 12.5px var(--mono);
  }
  .count {
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .problem {
    max-width: 45%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11.5px;
    color: var(--red);
  }
  .tabs {
    display: flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--field);
  }
  .tabs button {
    border: 0;
    background: none;
    color: var(--text-2);
    font-size: 12px;
    padding: 4px 12px;
    border-radius: 6px;
  }
  .tabs button.on {
    background: var(--surface);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
    color: var(--text);
    font-weight: 500;
  }
  .modes {
    margin-left: auto;
    display: flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--field);
  }
  .modes button {
    border: 0;
    background: none;
    color: var(--text-2);
    font-size: 12px;
    padding: 4px 10px;
    border-radius: 6px;
    white-space: nowrap;
  }
  .modes button.on {
    background: var(--surface);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
    font-weight: 500;
  }
  .modes button.on.act {
    color: var(--green);
  }
  .modes button.on.advise {
    color: var(--accent);
  }
  .modes button.on.off {
    color: var(--red);
  }
</style>
