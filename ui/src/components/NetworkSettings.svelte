<script lang="ts">
  // Where the UI and the proxy listen. Saving restarts shiftWire on the new
  // addresses and moves this page there.
  type Net = { api_addr: string; proxy_addr: string; api_token: string | null; proxy_password: string | null; wsl: boolean };
  type Side = { iface: string; custom: string; port: string };

  const IFACES = [
    { value: '127.0.0.1', label: 'This machine only (127.0.0.1)' },
    { value: '0.0.0.0', label: 'All interfaces (other machines, VMs)' },
    { value: 'custom', label: 'A specific address…' },
  ];

  let net = $state<Net | null>(null);
  let ui = $state<Side>({ iface: '127.0.0.1', custom: '', port: '7777' });
  let proxy = $state<Side>({ iface: '127.0.0.1', custom: '', port: '8080' });
  let problem = $state<string | null>(null);
  let restarting = $state<string | null>(null);

  /** `192.168.1.5:8080` / `[::1]:8080` -> the form's fields. */
  function split(addr: string): Side {
    const i = addr.lastIndexOf(':');
    const host = addr.slice(0, i).replace(/^\[|\]$/g, '');
    const known = host === '127.0.0.1' || host === '0.0.0.0';
    return { iface: known ? host : 'custom', custom: known ? '' : host, port: addr.slice(i + 1) };
  }
  function join(s: Side): string {
    const host = s.iface === 'custom' ? s.custom.trim() : s.iface;
    return host.includes(':') ? `[${host}]:${s.port}` : `${host}:${s.port}`;
  }

  async function load() {
    net = await fetch('/api/network').then((r) => r.json());
    ui = split(net!.api_addr);
    proxy = split(net!.proxy_addr);
  }
  $effect(() => {
    void load();
  });

  const changed = $derived(!!net && (join(ui) !== net.api_addr || join(proxy) !== net.proxy_addr));

  async function save() {
    problem = null;
    const body = { api_addr: join(ui), proxy_addr: join(proxy) };
    const res = await fetch('/api/network', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    if (!res.ok) {
      problem = await res.text();
      return;
    }
    // Same host as now unless the UI moved to one specific address.
    const host = ui.iface === 'custom' ? ui.custom.trim() : location.hostname;
    const url = `${location.protocol}//${host.includes(':') ? `[${host}]` : host}:${ui.port}/`;
    restarting = url;
    const deadline = Date.now() + 20_000;
    await new Promise((r) => setTimeout(r, 800)); // let the old process stop first
    while (Date.now() < deadline) {
      try {
        await fetch(`${url}api/health`, { mode: 'no-cors', cache: 'no-store' });
        location.href = url;
        return;
      } catch {
        await new Promise((r) => setTimeout(r, 500));
      }
    }
    problem = `shiftWire did not come back on ${url} within 20 s. Check \`shiftwire logs\`.`;
    restarting = null;
  }
</script>

<div class="card">
  <h2>Network</h2>
  <p>Where shiftWire listens. Saving restarts it on the new addresses (capturing pauses for a moment) and moves this page there.</p>

  {#snippet side(title: string, s: Side, hint: string)}
    <fieldset>
      <legend>{title}</legend>
      <label>Interface
        <select bind:value={s.iface}>
          {#each IFACES as i (i.value)}<option value={i.value}>{i.label}</option>{/each}
        </select>
      </label>
      {#if s.iface === 'custom'}
        <label>Address<input bind:value={s.custom} placeholder="192.168.56.1" spellcheck="false" /></label>
      {/if}
      <label>Port<input class="port" bind:value={s.port} inputmode="numeric" spellcheck="false" /></label>
      <p class="hint">{hint}</p>
    </fieldset>
  {/snippet}

  <div class="cols">
    {@render side('UI, API and MCP', ui, 'Other machines need the access token (shown below once enabled).')}
    {@render side('Proxy', proxy, 'Clients from other machines need the proxy password: http://NAME:PASSWORD@host:port.')}
  </div>

  {#if net?.wsl && (ui.iface !== '127.0.0.1' || proxy.iface !== '127.0.0.1')}
    <p class="warn">
      shiftWire runs in WSL2, which by default sits behind NAT: "all interfaces" are WSL's own, so Windows reaches
      it (localhost) but other machines and VMs on your network do not. To fix that, turn on WSL's mirrored networking:
      put <code>[wsl2]</code> and <code>networkingMode=mirrored</code> in <code>%UserProfile%\.wslconfig</code>,
      run <code>wsl --shutdown</code>, and allow the port in the Windows firewall.
    </p>
  {/if}

  {#if net?.api_token || net?.proxy_password}
    <dl>
      {#if net.api_token}
        <dt>UI token</dt><dd><code>{net.api_token}</code> &nbsp;open the UI once as <code>http://HOST:{split(net.api_addr).port}/?token=…</code></dd>
      {/if}
      {#if net.proxy_password}
        <dt>Proxy password</dt><dd><code>{net.proxy_password}</code></dd>
      {/if}
    </dl>
  {/if}

  <div class="row">
    <button class="primary" onclick={save} disabled={!changed || !!restarting}>Save and restart</button>
    {#if restarting}<span class="muted">Restarting… then opening {restarting}</span>{/if}
    {#if problem}<span class="bad">{problem}</span>{/if}
  </div>
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
  p {
    margin: 0 0 10px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  fieldset {
    margin: 0;
    padding: 10px 12px;
    border: 1px solid var(--hairline);
    border-radius: 10px;
    display: grid;
    gap: 8px;
    align-content: start;
  }
  legend {
    padding: 0 4px;
    font-size: 12.5px;
    font-weight: 600;
  }
  label {
    display: grid;
    gap: 4px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-2);
  }
  input,
  select {
    height: 28px;
    padding: 0 9px;
    border: 1px solid var(--hairline);
    border-radius: 7px;
    background: var(--field);
    color: var(--text);
    font: 12.5px var(--mono);
    outline: none;
  }
  select {
    font-family: inherit;
  }
  input:focus,
  select:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  .port {
    width: 110px;
  }
  .hint {
    margin: 0;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .warn {
    margin: 12px 0 0;
    padding: 8px 12px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--orange) 12%, transparent);
    color: var(--text);
    font-size: 12.5px;
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 14px;
    margin: 12px 0 0;
    font-size: 12.5px;
  }
  dt {
    color: var(--text-2);
  }
  dd {
    margin: 0;
    word-break: break-all;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 12px;
  }
  button {
    border: 0;
    border-radius: 8px;
    padding: 5px 14px;
    background: var(--accent);
    color: #fff;
    font-weight: 500;
  }
  button:disabled {
    opacity: 0.45;
  }
  .muted {
    font-size: 12px;
    color: var(--text-3);
  }
  .bad {
    color: var(--red);
  }
</style>
