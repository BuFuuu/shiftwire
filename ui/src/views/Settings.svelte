<script lang="ts">
  import { live } from '../lib/live.svelte';
  import { setAgentMode } from '../lib/api';
  import MatchReplace from '../components/MatchReplace.svelte';
  import NetworkSettings from '../components/NetworkSettings.svelte';

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

  const api = $derived(`http://${live.status?.api_addr ?? '127.0.0.1:7777'}`);
  const proxy = $derived(live.status?.proxy_addr ?? '127.0.0.1:8080');
</script>

<section class="settings">
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

  <MatchReplace />

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

    <h3>Connect an agent</h3>
    <dl>
      <dt>MCP (HTTP)</dt>
      <dd><code>{api}/mcp?agent=claude</code></dd>
      <dt>MCP (stdio)</dt>
      <dd><code>shiftwire mcp --agent claude</code> &nbsp;(starts shiftWire if needed)</dd>
      <dt>Its own tools</dt>
      <dd><code>HTTPS_PROXY=http://claude:x@{proxy}</code> &nbsp;plus the CA below</dd>
    </dl>
    <p class="hint">The name (here <code>claude</code>) labels every request the agent sends, so you can filter with <code>actor:claude</code>.</p>
  </div>

  <NetworkSettings />

  <div class="card">
    <h2>HTTPS certificate</h2>
    <p>Clients must trust the shiftWire CA so shiftWire can show HTTPS. <a href="/api/ca.crt" download>Download shiftwire-ca.crt</a> or open <code>http://shiftwire/</code> through the proxy for install steps.</p>
    <p class="hint">Fingerprint (SHA-256): <code class="fp">{live.status?.ca_fingerprint ?? '…'}</code></p>
  </div>
</section>

<style>
  .settings {
    flex: 1;
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
