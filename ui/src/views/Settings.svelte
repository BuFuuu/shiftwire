<script lang="ts">
  import { live, setShowClaude } from '../lib/live.svelte';
  import { setAgentMode } from '../lib/api';
  import MatchReplace from '../components/MatchReplace.svelte';
  import NetworkSettings from '../components/NetworkSettings.svelte';
  import MaskingSettings from '../components/MaskingSettings.svelte';
  import { nav, openSettings, type SettingsSection } from '../lib/nav.svelte';

  // Rule count for the section list (MatchReplace loads the rules themselves).
  let ruleCount = $state<number | null>(null);
  $effect(() => {
    void nav.settings; // recount after leaving Match & replace
    fetch('/api/rules').then((r) => r.json()).then((list: unknown[]) => (ruleCount = list.length));
  });

  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;
  const modeLabel = { act: 'AI sends', advise: 'AI advises', off: 'AI off' } as const;
  const sections = $derived<{ id: SettingsSection; label: string; summary: string; warn?: boolean }[]>([
    {
      id: 'scope',
      label: 'Scope',
      summary: live.scope?.include.length ? plural(live.scope.include.length, 'host pattern') : 'not set',
      warn: !live.scope?.include.length,
    },
    { id: 'rules', label: 'Match & replace', summary: ruleCount === null ? '…' : ruleCount ? plural(ruleCount, 'rule') : 'none' },
    { id: 'agents', label: 'AI agents', summary: modeLabel[live.agentMode] },
    {
      id: 'masking',
      label: 'Masking',
      summary: live.status?.masking.level && live.status.masking.level !== 'off'
        ? `${live.status.masking.level} · ${plural(live.status.masking.sites.length, 'site')}`
        : 'off',
    },
    { id: 'network', label: 'Network', summary: `proxy ${live.status?.proxy_addr ?? '…'}` },
    { id: 'certificate', label: 'HTTPS certificate', summary: 'install the CA' },
  ]);

  let include = $state('');
  let exclude = $state('');
  let saved = $state<string | null>(null);
  let problem = $state<string | null>(null);

  const fromScope = (s: { include: string[]; exclude: string[] }) => {
    include = s.include.join('\n');
    exclude = s.exclude.join('\n');
  };

  $effect(() => {
    fetch('/api/scope').then((r) => r.json()).then(fromScope);
  });

  async function save() {
    problem = saved = null;
    const lines = (t: string) => t.split('\n').map((l) => l.trim()).filter(Boolean);
    const res = await fetch('/api/scope', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ include: lines(include), exclude: lines(exclude) }),
    });
    if (res.ok) {
      fromScope(await res.json());
      saved = 'Saved. Agents are held to this scope from now on.';
    } else {
      problem = await res.text();
    }
  }

  const mcp = $derived(`http://${(live.status?.mcp_addr ?? '127.0.0.1:7778').replace(/^0\.0\.0\.0:/, '127.0.0.1:')}`);
  const proxy = $derived(live.status?.proxy_addr ?? '127.0.0.1:8080');
</script>

<div class="settings">
  <nav aria-label="Settings sections">
    {#each sections as s (s.id)}
      <button class:on={nav.settings === s.id} onclick={() => openSettings(s.id)}>
        <span class="label">{s.label}</span>
        <span class="summary" class:warn={s.warn}>{s.summary}</span>
      </button>
    {/each}
  </nav>

<section>
  {#if nav.settings === 'scope'}
  <div class="card">
    <h2>Scope{live.status ? ` of ${live.status.project}` : ""}</h2>
    <p>
      AI agents may only send requests to these hosts. Everything else is refused and shown in red in History.
      Your own browser and Repeater are never restricted. One pattern per line: <code>shop.test</code>, or
      <code>*.shop.test</code> for the domain and all its subdomains.
    </p>
    <div class="cols">
      <label>In scope<textarea bind:value={include} rows="5" spellcheck="false" placeholder="shop.test&#10;*.api.shop.test"></textarea></label>
      <label>Out of scope (wins)<textarea bind:value={exclude} rows="5" spellcheck="false" placeholder="admin.shop.test"></textarea></label>
    </div>
    <div class="row">
      <button class="primary" onclick={save}>Save scope</button>
      {#if saved}<span class="ok">{saved}</span>{/if}
      {#if problem}<span class="bad">{problem}</span>{/if}
    </div>
  </div>

  {:else if nav.settings === 'rules'}
  <MatchReplace />

  {:else if nav.settings === 'agents'}
  <div class="card">
    <h2>AI agents</h2>
    <p>Choose what agents may do. The same choice sits in the top right of every view.</p>
    <div class="modes">
      <label><input type="radio" name="mode" checked={live.agentMode === 'act'} onchange={() => setAgentMode('act')} />
        <b>AI sends</b>: agents send requests themselves, within the scope.</label>
      <label><input type="radio" name="mode" checked={live.agentMode === 'advise'} onchange={() => setAgentMode('advise')} />
        <b>AI advises</b>: agents read History and the Repeater and prepare requests as Repeater tabs; you review and send them.</label>
      <label><input type="radio" name="mode" checked={live.agentMode === 'off'} onchange={() => setAgentMode('off')} />
        <b>AI off</b>: every agent request is refused (and recorded).</label>
    </div>

    <h3>Claude traffic in History</h3>
    <div class="row toggle">
      <button class:primary={live.showClaude} onclick={() => setShowClaude(!live.showClaude)}>
        {live.showClaude ? 'Shown' : 'Hidden'}
      </button>
      <span class="hint">
        Requests to <code>anthropic.com</code>, <code>claude.ai</code> and <code>claude.com</code>, mostly an agent's own
        model calls. Hiding them only changes what History shows; nothing is deleted.
      </span>
    </div>

    <h3>Connect an agent</h3>
    <dl>
      <dt>MCP (HTTP)</dt>
      <dd><code>{mcp}/mcp?agent=claude</code></dd>
      <dt>MCP (stdio)</dt>
      <dd><code>shiftwire mcp --agent claude</code> &nbsp;(starts shiftWire if needed)</dd>
      <dt>Its own tools</dt>
      <dd><code>HTTPS_PROXY=http://claude:x@{proxy}</code> &nbsp;plus the CA below</dd>
    </dl>
    <p class="hint">The name (here <code>claude</code>) labels every request the agent sends, so you can filter with <code>actor:claude</code>.</p>
  </div>

  {:else if nav.settings === 'masking'}
  <MaskingSettings />

  {:else if nav.settings === 'network'}
  <NetworkSettings />

  {:else}
  <div class="card">
    <h2>HTTPS certificate</h2>
    <p>Clients must trust the shiftWire CA so shiftWire can show HTTPS. <a href="/api/ca.crt" download>Download shiftwire-ca.crt</a> or open <code>http://shiftwire/</code> through the proxy for install steps.</p>
    <p class="hint">Fingerprint (SHA-256): <code class="fp">{live.status?.ca_fingerprint ?? '…'}</code></p>
  </div>
  {/if}
</section>
</div>

<style>
  .settings {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 190px minmax(0, 1fr);
  }
  nav {
    padding: 16px 10px;
    border-right: 1px solid var(--hairline);
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: auto;
  }
  nav button {
    display: grid;
    gap: 1px;
    text-align: left;
    border: 0;
    border-radius: 8px;
    padding: 6px 10px;
    background: none;
  }
  nav button.on {
    background: var(--selected);
  }
  nav .label {
    font-size: 13px;
    font-weight: 500;
  }
  nav .summary {
    font-size: 11.5px;
    color: var(--text-3);
  }
  nav .summary.warn {
    color: var(--orange);
  }
  section {
    min-height: 0;
    overflow: auto;
    padding: 20px;
    display: grid;
    gap: 16px;
    align-content: start;
    max-width: 860px;
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
  h3 {
    margin: 16px 0 6px;
    font-size: 13px;
  }
  p {
    margin: 0 0 10px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .modes {
    display: grid;
    gap: 6px;
    margin-bottom: 4px;
  }
  .modes label {
    display: flex;
    gap: 8px;
    align-items: baseline;
    font-size: 13px;
    font-weight: 400;
    color: var(--text);
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  label {
    display: grid;
    gap: 5px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-2);
  }
  textarea {
    resize: vertical;
    padding: 8px 10px;
    border: 1px solid var(--hairline);
    border-radius: 8px;
    background: var(--field);
    color: var(--text);
    font: 12.5px/1.5 var(--mono);
    outline: none;
  }
  textarea:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 10px;
  }
  button {
    border: 1px solid var(--hairline);
    border-radius: 8px;
    padding: 5px 14px;
    background: var(--surface);
    color: var(--text);
  }
  button:hover {
    background: var(--hover);
  }
  .primary {
    border: 0;
    background: var(--accent);
    color: #fff;
    font-weight: 500;
  }
  .primary:hover {
    background: var(--accent);
    filter: brightness(1.08);
  }
  .toggle {
    margin: 0 0 4px;
  }
  .toggle button {
    min-width: 76px;
  }
  .toggle .hint {
    color: var(--text-2);
    line-height: 1.5;
  }
  .ok {
    color: var(--green);
  }
  .bad {
    color: var(--red);
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 14px;
    margin: 0;
    font-size: 12.5px;
  }
  dt {
    color: var(--text-2);
  }
  dd {
    margin: 0;
  }
  .hint {
    font-size: 12px;
  }
  .fp {
    word-break: break-all;
  }
  a {
    color: var(--accent);
    text-decoration: none;
  }
</style>
