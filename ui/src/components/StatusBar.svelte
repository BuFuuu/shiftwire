<script lang="ts">
  // Bottom bar: the UI, then the three ways into shiftWire (proxy, MCP, AI browser), each
  // in its own box. Green = running, red = error, gray = off. A box opens its settings.
  // The daemon pushes status changes (at most once a second).
  import Icon from './Icon.svelte';
  import { live } from '../lib/live.svelte';
  import { openSettings } from '../lib/nav.svelte';

  /** `0.0.0.0:8080` → `127.0.0.1:8080`: what a client on this machine uses. */
  const local = (addr: string) => addr.replace(/^0\.0\.0\.0:/, '127.0.0.1:').replace(/^\[::\]:/, '127.0.0.1:');
  let copied = $state<string | null>(null);
  async function copy(what: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Older browsers or no permission: the classic way.
      const area = Object.assign(document.createElement('textarea'), { value: text });
      document.body.append(area);
      area.select();
      document.execCommand('copy');
      area.remove();
    }
    copied = what;
    setTimeout(() => copied === what && (copied = null), 1200);
  }

  let now = $state(Date.now());
  let busy = $state(false);
  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  function ago(ms: number): string {
    const s = Math.max(0, Math.round((now - ms) / 1000));
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    return `${Math.floor(s / 3600)}h ago`;
  }

  /** Opens or closes the AI browser; the status push shows the result (and any error). */
  async function browser(action: 'open' | 'close') {
    busy = true;
    try {
      await fetch(`/api/browser/${action}`, { method: 'POST' });
    } finally {
      busy = false;
    }
  }

  const s = $derived(live.status);
  const proxyState = $derived(s?.proxy_error ? 'bad' : s?.proxy_running ? 'ok' : 'idle');
  const mcpState = $derived(s?.mcp_error ? 'bad' : s?.mcp_running ? 'ok' : 'idle');
  const b = $derived(s?.browser);
  const browserState = $derived(b?.running ? 'ok' : b?.error ? 'bad' : 'idle');
  const agents = $derived(s?.agent_list ?? []);
  const masking = $derived(s?.masking);
</script>

{#snippet copyButton(what: string, text: string)}
  <button class="copy" onclick={() => copy(what, text)} title={copied === what ? 'Copied' : `Copy ${text}`} aria-label={`Copy ${text}`}>
    <Icon name={copied === what ? 'check' : 'copy'} size={12} />
  </button>
{/snippet}

{#if s}
  <footer class="bar">
    <span class="seg ok">
      <button class="main" onclick={() => openSettings('network')} title="This window: the UI and REST API (Settings → Network)">
        <span class="dot"></span><b>UI</b> <code>{s.api_addr}</code>
      </button>
      {@render copyButton('ui', location.origin)}
    </span>

    <span class="ways" aria-label="Ways in">
      <span class="seg {proxyState}">
        <button class="main" onclick={() => openSettings('network')}
          title={s.proxy_error ?? 'Proxy port: point a browser, VM or tool here (Settings → Network)'}>
          <span class="dot"></span><b>Proxy</b> <code>{s.proxy_addr}</code>
          {#if s.proxy_error}<span class="note">down</span>{:else}
            <span class="note">{s.proxy_connections} open{s.proxy_last_request ? ` · last ${ago(s.proxy_last_request)}` : ''}</span>
          {/if}
        </button>
        {@render copyButton('proxy', `http://${local(s.proxy_addr)}`)}
      </span>

      <span class="seg {mcpState}">
        <button class="main" onclick={() => openSettings('agents')}
          title={s.mcp_error ?? (agents.length
            ? `Agents (last 10 minutes):\n${agents.map((a) => `${a.name}  ${ago(a.last_seen)}`).join('\n')}`
            : 'MCP port for AI agents (Settings → AI agents)')}>
          <span class="dot"></span><b>MCP</b> <code>{s.mcp_addr}</code>
          {#if s.mcp_error}<span class="note">down</span>{:else if agents.length}
            <span class="note">{agents[0].name} {ago(agents[0].last_seen)}{agents.length > 1 ? ` +${agents.length - 1}` : ''}</span>
          {:else}<span class="note">no agent yet</span>{/if}
        </button>
        {@render copyButton('mcp', `http://${local(s.mcp_addr)}/mcp`)}
      </span>

      <span class="seg {browserState}"
        title={b?.error ?? (b?.running ? `AI browser: ${b.binary}, through its own proxy port ${b.proxy_port}` : 'A browser shiftWire starts for you, routed through shiftWire, with this project\'s cookies and logins')}>
        <span class="dot"></span><b>AI browser</b>
        {#if b?.running}
          <span class="note">open</span>
          <button class="act" disabled={busy} onclick={() => browser('close')}>Close</button>
        {:else}
          {#if b?.error}<span class="note">could not open</span>{/if}
          <button class="act primary" disabled={busy} onclick={() => browser('open')}>Open</button>
        {/if}
      </span>

    </span>

    {#if masking && masking.level !== 'off'}
      <button class="seg mask" onclick={() => openSettings('masking')}
        title={`Masking (${masking.level}): agents see ${masking.sites.map((x) => `${x.alias} for ${x.real}`).join(', ') || 'no sites yet'}`}>
        <span class="dot"></span><b>Masking</b> <span class="note">{masking.level} · {masking.sites.length} site{masking.sites.length === 1 ? '' : 's'}</span>
      </button>
    {/if}

    <span class="right">
      <span title={s.project_path}>{s.project}</span>
      · {s.flows.toLocaleString()} requests
      {#if s.dropped}<span class="dropped" title="Captures skipped because storage could not keep up (traffic was still forwarded)">· {s.dropped} dropped</span>{/if}
    </span>
  </footer>
{/if}

<style>
  .bar {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    gap: 10px;
    height: 28px;
    padding: 0 8px;
    border-top: 1px solid var(--hairline);
    background: var(--sidebar);
    color: var(--text-2);
    font-size: 11.5px;
    white-space: nowrap;
    overflow: hidden;
  }
  .ways {
    display: flex;
    align-items: center;
    gap: 6px;
    padding-left: 10px;
    border-left: 1px solid var(--hairline);
  }
  .seg {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 21px;
    padding: 0 9px;
    border: 1px solid var(--hairline);
    border-radius: 6px;
    background: var(--surface);
    color: inherit;
    font: inherit;
  }
  button.seg:hover,
  .seg:has(.main:hover) {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--hairline));
  }
  .main {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 100%;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
  }
  .copy {
    display: flex;
    align-items: center;
    margin-right: -4px;
    padding: 2px 3px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--text-3);
  }
  .copy:hover {
    background: var(--hover);
    color: var(--text);
  }
  b {
    font-weight: 600;
    color: var(--text);
  }
  code {
    font: 11px var(--mono);
    color: var(--text);
  }
  .note {
    color: var(--text-3);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-3);
  }
  .ok .dot {
    background: var(--green);
  }
  .bad .dot {
    background: var(--red);
  }
  .bad .note {
    color: var(--red);
  }
  .warn .dot {
    background: var(--orange);
  }
  .mask .dot {
    background: var(--ic-timeline);
  }
  .act {
    height: 16px;
    padding: 0 7px;
    border: 1px solid var(--hairline);
    border-radius: 4px;
    background: none;
    color: var(--text-2);
    font: 500 10.5px var(--font);
  }
  .act.primary {
    border-color: transparent;
    background: var(--accent);
    color: #fff;
  }
  .act:disabled {
    opacity: 0.5;
  }
  .right {
    margin-left: auto;
    padding: 0 8px;
  }
  .dropped {
    color: var(--orange);
  }
</style>
