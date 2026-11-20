<script lang="ts">
  // Match & replace / alert rules for every request passing the proxy (a card
  // in Settings). They belong to the current project, like the scope.
  import { live } from '../lib/live.svelte';
  import type { Rule } from '../lib/types';

  let rules = $state<Rule[]>([]);
  let saved = $state<string | null>(null);
  let problem = $state<string | null>(null);
  let dirty = $state(false);

  $effect(() => {
    fetch('/api/rules').then((r) => r.json()).then((list: Rule[]) => (rules = list));
  });

  const blank = (over: Partial<Rule>): Rule => ({
    id: '',
    name: '',
    enabled: true,
    mode: 'replace',
    part: 'headers',
    pattern: '',
    replace: '',
    when_missing: false,
    block: false,
    ...over,
  });

  const examples: { label: string; rule: Partial<Rule> }[] = [
    { label: 'Set my User-Agent', rule: { name: 'my user agent', pattern: '^User-Agent: .*$', replace: 'User-Agent: Mozilla/5.0 (pentest)' } },
    { label: 'Alert: User-Agent is not mine', rule: { name: 'foreign user agent', mode: 'alert', pattern: '^User-Agent: Mozilla/5.0 \\(pentest\\)$', when_missing: true } },
    { label: 'Remove a header', rule: { name: 'drop debug header', pattern: '^X-Debug: .*$', replace: '' } },
    { label: 'Alert: password in a body', rule: { name: 'password sent', mode: 'alert', part: 'body', pattern: 'password=' } },
    { label: 'Block: password in a body', rule: { name: 'never send passwords', mode: 'alert', part: 'body', pattern: 'password=', block: true } },
  ];

  function add(over: Partial<Rule> = {}) {
    rules = [...rules, blank(over)];
    dirty = true;
  }

  function remove(i: number) {
    rules = rules.filter((_, j) => j !== i);
    dirty = true;
  }

  function move(i: number, by: number) {
    const j = i + by;
    if (j < 0 || j >= rules.length) return;
    const next = [...rules];
    [next[i], next[j]] = [next[j], next[i]];
    rules = next;
    dirty = true;
  }

  async function save() {
    saved = problem = null;
    const res = await fetch('/api/rules', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(rules),
    });
    if (res.ok) {
      rules = await res.json();
      dirty = false;
      saved = 'Saved. Applied to every request from now on.';
    } else {
      problem = await res.text();
    }
  }
</script>

<div class="rules" oninput={() => ((dirty = true), (saved = null))} role="group">
  <div class="card">
    <h2>Match & Replace{live.status ? ` · ${live.status.project}` : ''}</h2>
    <p>
      Every request passing the proxy is checked against these rules, top to bottom, before it is forwarded.
      <b>Replace</b> rewrites what matches; <b>Alert</b> changes nothing but tags the request and pops up a notice.
      Patterns are regular expressions. Header rules see one <code>Name: value</code> line per header
      (<code>^</code> and <code>$</code> match per line); replacing a whole header line with nothing removes it.
      Body rules see uncompressed bodies up to 2 MB.
    </p>

    {#if rules.length === 0}
      <p class="empty">No rules yet. Start from an example or add an empty rule.</p>
    {/if}

    <div class="list">
      {#each rules as rule, i (i)}
        <div class="rule" class:off={!rule.enabled}>
          <div class="top">
            <input type="checkbox" bind:checked={rule.enabled} title="Rule on/off" onchange={() => (dirty = true)} />
            <input class="name" bind:value={rule.name} placeholder="Name" spellcheck="false" />
            <div class="seg" role="radiogroup" aria-label="Mode">
              <button class:on={rule.mode === 'replace'} onclick={() => ((rule.mode = 'replace'), (dirty = true))}>Replace</button>
              <button class:on={rule.mode === 'alert'} onclick={() => ((rule.mode = 'alert'), (dirty = true))}>Alert</button>
            </div>
            <select bind:value={rule.part} onchange={() => (dirty = true)} aria-label="Where">
              <option value="headers">in headers</option>
              <option value="body">in body</option>
            </select>
            <span class="spacer"></span>
            <button class="icon" onclick={() => move(i, -1)} disabled={i === 0} title="Move up">↑</button>
            <button class="icon" onclick={() => move(i, 1)} disabled={i === rules.length - 1} title="Move down">↓</button>
            <button class="icon danger" onclick={() => remove(i)} title="Delete rule">✕</button>
          </div>
          <div class="fields">
            <label>{rule.mode === 'replace' ? 'Find' : 'Pattern'}
              <input class="mono" bind:value={rule.pattern} placeholder="^User-Agent: .*$" spellcheck="false" />
            </label>
            {#if rule.mode === 'replace'}
              <label>Replace with <em>($1 = first group; empty removes)</em>
                <input class="mono" bind:value={rule.replace} placeholder="User-Agent: mine" spellcheck="false" />
              </label>
            {:else}
              <label>Alert when
                <select bind:value={rule.when_missing} onchange={() => (dirty = true)}>
                  <option value={false}>the pattern matches</option>
                  <option value={true}>the pattern is missing</option>
                </select>
              </label>
              <label class="check" title="shiftWire answers 403 and the request never reaches the target">
                <input type="checkbox" bind:checked={rule.block} onchange={() => (dirty = true)} /> Block the request
              </label>
            {/if}
          </div>
        </div>
      {/each}
    </div>

    <div class="row">
      <button onclick={() => add()}>+ Add rule</button>
      <span class="examples">
        Examples:
        {#each examples as ex (ex.label)}
          <button class="link" onclick={() => add(ex.rule)}>{ex.label}</button>
        {/each}
      </span>
    </div>
    <div class="row">
      <button class="primary" onclick={save} disabled={!dirty}>Save rules</button>
      {#if dirty}<span class="muted">Unsaved changes</span>{/if}
      {#if saved}<span class="ok">{saved}</span>{/if}
      {#if problem}<span class="bad">{problem}</span>{/if}
    </div>
  </div>
  <p class="hint">
    Changed requests get the tag <code>replaced:&lt;name&gt;</code>, alerts <code>alert:&lt;name&gt;</code>: filter History with
    <code>tag:alert*</code>. History shows each request as it was sent. A blocking alert stops the request (shiftWire
    answers 403; History shows it in red). Agents' own MCP sends obey alert and block rules too; replace rules change only
    proxied requests, and your Repeater is never changed.
  </p>
</div>

<style>
  .rules {
    display: grid;
    gap: 8px;
  }
  .card {
    padding: 16px 18px;
    border: 1px solid var(--hairline);
    border-radius: 12px;
    background: var(--surface);
  }
  h2 {
    margin: 0 0 6px;
    font-size: 15px;
  }
  p {
    margin: 0 0 12px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .empty {
    color: var(--text-3);
  }
  .list {
    display: grid;
    gap: 8px;
  }
  .rule {
    padding: 10px 12px;
    border: 1px solid var(--hairline);
    border-radius: 10px;
    display: grid;
    gap: 8px;
  }
  .rule.off {
    opacity: 0.55;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .spacer {
    flex: 1;
  }
  .fields {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  label {
    display: grid;
    gap: 4px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-2);
  }
  label.check {
    display: flex;
    align-items: center;
    gap: 6px;
    align-self: end;
    height: 28px;
  }
  em {
    font-style: normal;
    font-weight: 400;
    color: var(--text-3);
  }
  input:not([type='checkbox']),
  select {
    height: 28px;
    padding: 0 9px;
    border: 1px solid var(--hairline);
    border-radius: 7px;
    background: var(--field);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
    outline: none;
  }
  input:focus,
  select:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  .name {
    width: 200px;
    font-weight: 500;
  }
  .mono {
    font-family: var(--mono);
  }
  .seg {
    display: inline-flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--field);
  }
  .seg button {
    padding: 2px 10px;
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
  button {
    border: 1px solid var(--hairline);
    border-radius: 8px;
    padding: 4px 12px;
    background: var(--surface);
    color: var(--text);
  }
  button:hover:not(:disabled) {
    background: var(--hover);
  }
  button:disabled {
    opacity: 0.45;
  }
  .icon {
    padding: 2px 8px;
  }
  .danger {
    color: var(--red);
  }
  .primary {
    border: 0;
    background: var(--accent);
    color: #fff;
    font-weight: 500;
  }
  .primary:hover:not(:disabled) {
    background: var(--accent);
    filter: brightness(1.08);
  }
  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 12px;
  }
  .examples {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px;
    font-size: 12px;
    color: var(--text-3);
  }
  .link {
    border: 0;
    padding: 2px 6px;
    background: none;
    color: var(--accent);
    font-size: 12px;
  }
  .muted {
    font-size: 12px;
    color: var(--text-3);
  }
  .ok {
    color: var(--green);
  }
  .bad {
    color: var(--red);
  }
  .hint {
    margin: 0 4px;
    font-size: 12px;
    color: var(--text-3);
  }
</style>
