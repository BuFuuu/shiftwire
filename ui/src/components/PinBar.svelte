<script lang="ts">
  // Lasting filters: "hide *mozilla*", "show only shop.test". They stay until
  // removed, and apply on top of whatever is typed in the main filter.
  import { addPin, effectiveFilter, hasScope, history, idRange, live, setPins, setScopeOnly, type Pin } from '../lib/live.svelte';

  // Exports exactly what History shows (main filter + chips).
  const harHref = $derived(`/api/export/har?filter=${encodeURIComponent(effectiveFilter())}`);

  let adding = $state<Pin['mode'] | null>(null);
  let text = $state('');
  let input: HTMLInputElement | undefined = $state();

  $effect(() => {
    if (adding) input?.focus();
  });

  function add(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      adding = null;
      text = '';
    }
    if (e.key !== 'Enter' || !adding) return;
    addPin(adding, text);
    text = '';
    adding = null;
  }

  // Clear: hide every request up to the newest # shown; new ones keep coming.
  // Nothing is deleted: remove the chip and they are back.
  const newest = $derived(history.flows.reduce((a, f) => (!a || f.seq > a.seq ? f : a), history.flows[0]));
  function clear() {
    if (!newest) return;
    const pin: Pin = { text: `1-${newest.seq}`, mode: 'after', on: true, label: `#1–${newest.seq}` };
    setPins([...live.pins.filter((p) => p.mode !== 'after'), pin]);
  }

  const toggle = (i: number) => setPins(live.pins.map((p, j) => (j === i ? { ...p, on: !p.on } : p)));
  const remove = (i: number) => setPins(live.pins.filter((_, j) => j !== i));

  // Right-click a chip to change it in place: Enter saves, Esc cancels.
  let editing = $state<number | null>(null);
  let editText = $state('');
  function startEdit(e: MouseEvent, i: number) {
    e.preventDefault();
    editing = i;
    const pin = live.pins[i];
    const range = pin.mode === 'after' ? idRange(pin.text) : null;
    editText = range ? `${range[0]}-${range[1]}` : pin.text;
  }
  function editKey(e: KeyboardEvent) {
    if (e.key === 'Escape') editing = null;
    if (e.key !== 'Enter' || editing === null) return;
    const value = editText.trim().replaceAll('"', '');
    const i = editing;
    editing = null;
    if (!value) return remove(i);
    const pin = live.pins[i];
    let text = value;
    let label: string | undefined;
    if (pin.mode === 'after') {
      // A Clear chip is a range of #: edit both ends, e.g. 20-50.
      const range = idRange(value);
      if (!range) return;
      text = `${range[0]}-${range[1]}`;
      label = `#${range[0]}–${range[1]}`;
    }
    setPins(live.pins.map((p, j) => (j === i ? { ...p, text, label, on: true } : p)));
  }
  const focus = (el: HTMLInputElement) => el.select();
</script>

<div class="pins" aria-label="Lasting filters">
  {#each live.pins as pin, i (pin.mode + pin.text)}
    {#if editing === i}
      <input class="edit" bind:value={editText} onkeydown={editKey} onblur={() => (editing = null)} use:focus spellcheck="false" />
    {:else}
    <span class="pin {pin.mode}" class:off={!pin.on} oncontextmenu={(e) => startEdit(e, i)} role="group">
      <button class="label" onclick={() => toggle(i)} title={pin.on ? 'Click to pause · right-click to change' : 'Paused: click to apply again · right-click to change'}>
        <span class="icon">{pin.mode === 'show' ? '✓' : '⊘'}</span>{pin.label ?? pin.text}
      </button>
      <button class="x" onclick={() => remove(i)} aria-label="Remove filter" title="Remove">×</button>
    </span>
    {/if}
  {/each}
  {#if adding}
    <input
      bind:this={input}
      bind:value={text}
      onkeydown={add}
      onblur={() => (adding = null)}
      spellcheck="false"
      placeholder={adding === 'hide' ? 'Hide rows containing… (* = anything, or status:3*)' : 'Show only rows containing… (or host:*.shop.test)'}
    />
  {:else}
    <button class="add" onclick={() => (adding = 'hide')} title="Always hide rows that contain this text, e.g. *mozilla*">+ Hide…</button>
    <button class="add" onclick={() => (adding = 'show')} title="Only show rows that contain this text (several: any of them)">+ Show only…</button>
    <button
      class="scope-only"
      class:on={live.scopeOnly && hasScope()}
      onclick={() => setScopeOnly(!live.scopeOnly)}
      disabled={!hasScope()}
      title={hasScope() ? 'Show only requests to hosts in scope (Settings)' : 'Set a scope in Settings first'}
    >
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="4" /></svg>
      In scope
    </button>
    <button class="add" onclick={clear} disabled={!newest} title="Hide every request up to now; new ones keep appearing. Nothing is deleted: remove the chip to see them again.">Clear</button>
  {/if}
  <a class="export" href={harHref} download title="Download the requests shown here as a HAR file (for browsers, Burp, ZAP)">Export HAR</a>
</div>

<style>
  .pins {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 16px;
    border-bottom: 1px solid var(--hairline);
    min-height: 34px;
  }
  .pin {
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    font: 11.5px var(--mono);
  }
  .pin.hide,
  .pin.after {
    background: color-mix(in srgb, var(--red) 12%, transparent);
    color: var(--red);
  }
  .pin.show {
    background: color-mix(in srgb, var(--green) 14%, transparent);
    color: var(--green);
  }
  .pin.off {
    opacity: 0.45;
    text-decoration: line-through;
  }
  .pin button {
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
  }
  .label {
    padding: 2px 4px 2px 10px;
  }
  .icon {
    margin-right: 5px;
  }
  .x {
    padding: 2px 9px 2px 3px;
    opacity: 0.7;
  }
  .x:hover {
    opacity: 1;
  }
  .add {
    border: 1px dashed var(--hairline);
    border-radius: 999px;
    padding: 2px 10px;
    background: none;
    color: var(--text-2);
    font-size: 11.5px;
  }
  .scope-only {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 10px;
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
  .add:disabled {
    opacity: 0.45;
  }
  .add:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--text-3);
  }
  .export {
    margin-left: auto;
    font-size: 11.5px;
    color: var(--text-2);
    text-decoration: none;
  }
  .export:hover {
    color: var(--accent);
  }
  input.edit {
    width: 200px;
  }
  input {
    width: 260px;
    height: 24px;
    padding: 0 10px;
    border: 1px solid color-mix(in srgb, var(--accent) 55%, transparent);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text);
    font: 11.5px var(--mono);
    outline: none;
  }
</style>
