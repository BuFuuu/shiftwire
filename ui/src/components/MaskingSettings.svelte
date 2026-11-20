<script lang="ts">
  // Masking: agents see stand-in names (target1.masked) instead of the target's real
  // names, in everything they get over MCP. Settings for the loaded project.
  import { live } from '../lib/live.svelte';
  import type { Masking, MaskLevel } from '../lib/types';

  let masking = $state<Masking>({ level: 'off', sites: [], words: [], allow: [], auto: [] });
  // One per line in the text areas.
  let words = $state('');
  let allow = $state('');
  const lines = (t: string) => t.split('\n').map((l) => l.trim()).filter(Boolean);
  function show(m: Masking) {
    masking = m;
    words = m.words.join('\n');
    allow = m.allow.join('\n');
  }
  let newSite = $state('');
  let dirty = $state(false);
  let saved = $state<string | null>(null);
  let problem = $state<string | null>(null);

  $effect(() => {
    fetch('/api/masking').then((r) => r.json()).then(show);
  });

  const levels: { id: MaskLevel; label: string; help: string }[] = [
    { id: 'off', label: 'Off', help: 'Agents see the real names.' },
    {
      id: 'addresses',
      label: 'Addresses',
      help: 'Everything shiftWire writes itself, and in target content the addresses: URLs, Host / Origin / Referer / Location / cookie headers, e-mail addresses. Page text and body values mentioning the name stay as they are. For stable aliases, not secrecy.',
    },
    {
      id: 'content',
      label: 'Content',
      help: 'Every occurrence of a masked name, anywhere: no masked name ever reaches an agent as plain text. Screenshots, browser_evaluate and images are off.',
    },
    {
      id: 'strict',
      label: 'Strict',
      help: 'Content, plus: your words below become [word1], [word2], …; every other host agents could see gets an alias too (h1.masked, kept per project), except the allow-list; base64 values that encode a masked name (e.g. a JWT naming the host) are withheld.',
    },
  ];

  /** `*.shop.example.co.uk` → `example.co.uk` (IPs have no site). */
  function siteOf(pattern: string): string | null {
    const host = pattern.replace(/^\*\./, '').toLowerCase();
    if (/^[\d.]+$/.test(host) || host.includes(':') || !host.includes('.')) return null;
    const labels = host.split('.');
    const n = labels.length;
    const take = n >= 3 && labels[n - 1].length === 2 && labels[n - 2].length <= 3 ? 3 : 2;
    return labels.slice(-take).join('.');
  }

  const covered = (host: string) =>
    masking.sites.some((s) => host === s.real || host.endsWith(`.${s.real}`));
  /** In scope but not masked: agents would see these names. */
  const uncovered = $derived(
    (live.scope?.include ?? []).map((p) => p.replace(/^\*\./, '').toLowerCase()).filter((h) => siteOf(h) && !covered(h)),
  );

  function add(real: string) {
    real = real.trim().toLowerCase();
    if (!real || masking.sites.some((s) => s.real === real)) return;
    masking.sites = [...masking.sites, { real, alias: '' }];
    dirty = true;
  }

  function addFromScope() {
    for (const host of uncovered) {
      const site = siteOf(host);
      if (site) add(site);
    }
  }

  function remove(i: number) {
    masking.sites = masking.sites.filter((_, j) => j !== i);
    dirty = true;
  }

  async function save() {
    problem = saved = null;
    const res = await fetch('/api/masking', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ...masking, words: lines(words), allow: lines(allow) }),
    });
    if (res.ok) {
      show(await res.json());
      dirty = false;
      saved = masking.level === 'off' ? 'Saved. Masking is off.' : 'Saved. Agents get the new names with their next call.';
    } else {
      problem = await res.text();
    }
  }
</script>

<div class="card">
  <h2>Masking</h2>
  <p>
    Agents see stand-in names instead of the target's real ones, in everything they get over MCP, so the real names
    are not sent to the AI provider. They write the stand-ins back, and shiftWire sends to the real hosts. A site also
    covers its subdomains (<code>api.example.com</code> → <code>api.target1.masked</code>). You and History always see
    the real names.
  </p>

  <div class="levels" role="radiogroup" aria-label="Masking level">
    {#each levels as l (l.id)}
      <label class="level" class:on={masking.level === l.id}>
        <input type="radio" name="level" value={l.id} bind:group={masking.level} onchange={() => (dirty = true)} />
        <span><b>{l.label}</b><span class="help">{l.help}</span></span>
      </label>
    {/each}
  </div>

  <h3>Masked sites</h3>
  {#if masking.sites.length}
    <div class="sites">
      {#each masking.sites as site, i (site.real)}
        <code class="real">{site.real}</code>
        <span class="arrow">→</span>
        <input class="mono" bind:value={site.alias} placeholder="next targetN.masked" oninput={() => (dirty = true)} spellcheck="false" />
        <button class="icon" onclick={() => remove(i)} title="Remove">✕</button>
      {/each}
    </div>
  {:else}
    <p class="muted">No sites yet.</p>
  {/if}
  <div class="row">
    <input class="mono add" bind:value={newSite} placeholder="example.com" spellcheck="false"
      onkeydown={(e) => e.key === 'Enter' && (add(newSite), (newSite = ''))} />
    <button onclick={() => (add(newSite), (newSite = ''))}>Add site</button>
    {#if uncovered.length}<button onclick={addFromScope}>Add from scope</button>{/if}
  </div>
  {#if uncovered.length && masking.level !== 'off'}
    <p class="warn">In scope but not masked, so agents see these names: {uncovered.join(', ')}</p>
  {/if}

  <h3>Strict only</h3>
  <div class="strict" class:dim={masking.level !== 'strict'}>
    <label>Words to mask <em>(one per line: company, product, people, subdomain labels; at least 3 characters)</em>
      <textarea bind:value={words} oninput={() => (dirty = true)} rows="3" spellcheck="false" placeholder="Acme"></textarea>
    </label>
    <label>Hosts agents may see as they are <em>(one per line, *.example.com for a domain and its subdomains)</em>
      <textarea bind:value={allow} oninput={() => (dirty = true)} rows="3" spellcheck="false" placeholder="*.gstatic.com"></textarea>
    </label>
  </div>
  {#if masking.auto.length}
    <p class="muted">Other hosts with an alias so far ({masking.auto.length}):
      {masking.auto.map((a) => `${a.alias} = ${a.real}`).join(', ')}
      <button class="link" onclick={() => ((masking.auto = []), (dirty = true))}>Forget them</button>
    </p>
  {/if}

  <div class="row">
    <button class="primary" onclick={save} disabled={!dirty}>Save</button>
    {#if dirty}<span class="muted">Unsaved changes</span>{/if}
    {#if saved}<span class="ok">{saved}</span>{/if}
    {#if problem}<span class="bad">{problem}</span>{/if}
  </div>

  <h3>What masking does not do</h3>
  <ul class="limits">
    <li><b>Not a sandbox.</b> It filters what goes through the MCP port. An agent with a shell on this machine can read the project database, the REST API and the browser profile directly. For real separation, run the agent elsewhere with only the MCP port reachable.</li>
    <li><b>Only MCP.</b> What you type or paste into the AI chat, files the agent reads, and its other tools (web search, another browser) are outside shiftWire.</li>
    <li><b>Encoded names are not masked:</b> hashes, compressed or binary data, images and PDFs, punycode, <code>&#92;u0065</code> escapes, names built by JavaScript. Strict only finds base64 / base64url (and withholds the whole value).</li>
    <li><b>The target can be guessed without its name:</b> page text, product and people names, logos, unique paths, the tech stack.</li>
    <li><b>Other sites can name the target</b> (<code>example.zendesk.com</code>, <code>example.okta.com</code>) unless you mask them too. Strict aliases the hosts it sees in addresses and the scope; a host only mentioned in page text stays.</li>
    <li><b>Words replace too much when they are common:</b> a short or everyday word in the Strict list also replaces unrelated text. Words are matched in any case (also inside paths like <code>/css/acme_site.css</code>), and what an agent writes back gets the spelling from your list, so it may miss a case-sensitive path; <code>repeat_flow</code> keeps the real request.</li>
    <li><b>Addresses leaves page text alone by design;</b> it is for stable aliases, not secrecy.</li>
    <li><b>What agents see is not byte-exact:</b> lengths change, signatures over a body no longer match. <code>repeat_flow</code> works on the real request.</li>
    <li><b>Less to work with:</b> agents cannot route their own tools through the proxy while masking is on; at Content and Strict, no screenshots or <code>browser_evaluate</code>.</li>
    <li><b>Not retroactive:</b> names an agent saw before stay in its context. Start a new agent session after turning masking on.</li>
  </ul>
</div>

<style>
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
    margin: 16px 0 8px;
    font-size: 12.5px;
  }
  p {
    margin: 0 0 10px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .levels {
    display: grid;
    gap: 6px;
  }
  .level {
    display: flex;
    gap: 9px;
    align-items: flex-start;
    padding: 8px 10px;
    border: 1px solid var(--hairline);
    border-radius: 8px;
    font-size: 12.5px;
  }
  .level.on {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--hairline));
    background: var(--selected);
  }
  .level input {
    margin-top: 2px;
  }
  .help {
    display: block;
    margin-top: 2px;
    color: var(--text-2);
    line-height: 1.45;
  }
  .sites {
    display: grid;
    grid-template-columns: max-content auto minmax(160px, 260px) auto;
    gap: 6px 10px;
    align-items: center;
  }
  .real {
    font: 12.5px var(--mono);
  }
  .arrow {
    color: var(--text-3);
  }
  input.mono {
    height: 28px;
    padding: 0 9px;
    border: 1px solid var(--hairline);
    border-radius: 7px;
    background: var(--field);
    color: var(--text);
    font: 12.5px var(--mono);
    outline: none;
  }
  input.add {
    width: 220px;
  }
  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    margin-top: 10px;
  }
  button {
    border: 1px solid var(--hairline);
    border-radius: 8px;
    padding: 4px 12px;
    background: none;
    color: var(--text);
  }
  button.primary {
    border-color: transparent;
    background: var(--accent);
    color: #fff;
    font-weight: 500;
  }
  button:disabled {
    opacity: 0.45;
  }
  button.icon {
    justify-self: start;
    padding: 2px 8px;
    color: var(--text-2);
  }
  .strict {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .strict.dim {
    opacity: 0.6;
  }
  .strict label {
    display: grid;
    gap: 4px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-2);
  }
  em {
    font-style: normal;
    font-weight: 400;
    color: var(--text-3);
  }
  textarea {
    padding: 6px 9px;
    border: 1px solid var(--hairline);
    border-radius: 7px;
    background: var(--field);
    color: var(--text);
    font: 12.5px var(--mono);
    resize: vertical;
    outline: none;
  }
  button.link {
    border: 0;
    padding: 0 4px;
    color: var(--accent);
  }
  .warn {
    margin-top: 10px;
    padding: 8px 12px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--orange) 12%, transparent);
    color: var(--text);
    font-size: 12.5px;
  }
  .muted {
    color: var(--text-3);
    font-size: 12px;
  }
  .ok {
    color: var(--green);
    font-size: 12px;
  }
  .bad {
    color: var(--red);
    font-size: 12px;
  }
  .limits {
    margin: 0;
    padding-left: 18px;
    color: var(--text-2);
    font-size: 12.5px;
    line-height: 1.5;
  }
  .limits li {
    margin-bottom: 4px;
  }
  .limits b {
    color: var(--text);
  }
</style>
