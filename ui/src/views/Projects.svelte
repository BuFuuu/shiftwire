<script lang="ts">
  // Projects are folders (project.json + flows.db), like in ShiftGrid. Switching
  // reloads every open window, because everything shown belongs to a project.
  import { live } from '../lib/live.svelte';
  import { bytes } from '../lib/format';
  import type { Project, ProjectInfo } from '../lib/types';

  let list = $state<ProjectInfo[]>([]);
  let dir = $state('');
  let problem = $state<string | null>(null);
  let busy = $state(false);

  let name = $state('');
  let scope = $state('');
  let folder = $state('');
  let openPath = $state('');

  async function load() {
    const res = await fetch('/api/projects');
    const data: { current: Project; dir: string; projects: ProjectInfo[] } = await res.json();
    list = data.projects;
    dir = data.dir;
  }
  $effect(() => {
    void load();
  });

  /** POSTs and, on success, waits for the project_changed event to reload the page. */
  async function post(path: string, body: object): Promise<boolean> {
    problem = null;
    busy = true;
    const res = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    busy = false;
    if (!res.ok) problem = await res.text();
    return res.ok;
  }

  function create(e: SubmitEvent) {
    e.preventDefault();
    const include = scope.split('\n').map((l) => l.trim()).filter(Boolean);
    void post('/api/projects', { name, folder, scope: { include, exclude: [] } });
  }

  const open = (path: string) => void post('/api/projects/switch', { path });

  async function remove(p: ProjectInfo) {
    if (!confirm(`Delete "${p.name}" and everything captured in it?\n\n${p.path}\n\nThis cannot be undone.`)) return;
    if (await post('/api/projects/delete', { path: p.path })) await load();
  }

  const date = (ms: number) => new Date(ms).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
</script>

<section class="projects">
  <div class="card">
    <h2>New project</h2>
    <form onsubmit={create}>
      <label>Name<input bind:value={name} required placeholder="ACME web app" spellcheck="false" /></label>
      <label>Scope <em>(hosts agents may test, one per line; editable later in Settings)</em>
        <textarea bind:value={scope} rows="3" spellcheck="false" placeholder="shop.test&#10;*.api.shop.test"></textarea>
      </label>
      <label>Folder <em>(where the project folder is created)</em>
        <input bind:value={folder} placeholder={dir} spellcheck="false" />
      </label>
      <div class="row">
        <button class="primary" type="submit" disabled={busy || !name.trim()}>Create and switch</button>
      </div>
    </form>
  </div>

  {#if problem}<p class="bad">{problem}</p>{/if}

  <div class="card">
    <div class="head">
      <h2>Projects</h2>
      <span class="muted">{list.length} found</span>
    </div>
    <div class="list">
      {#each list as p (p.path)}
        <article class:loaded={p.loaded}>
          <div class="main">
            <div class="title">{p.name} {#if p.loaded}<span class="pill current">current</span>{/if}</div>
            <code class="path">{p.path}</code>
            <div class="meta">
              <span class="pill">{(p.flows ?? 0).toLocaleString()} requests</span>
              <span class="pill">{bytes(p.size)}</span>
              <span class="pill" title="Created">{date(p.created_at)}</span>
            </div>
          </div>
          <div class="actions">
            {#if p.loaded}
              <button disabled>Loaded</button>
            {:else}
              <button class="primary" disabled={busy} onclick={() => open(p.path)}>Switch</button>
              <button class="danger" disabled={busy} onclick={() => remove(p)}>Delete</button>
            {/if}
          </div>
        </article>
      {/each}
    </div>
    <form class="row" onsubmit={(e) => (e.preventDefault(), open(openPath))}>
      <input bind:value={openPath} placeholder="Open a project folder elsewhere: /path/to/project" spellcheck="false" />
      <button type="submit" disabled={busy || !openPath.trim()}>Open</button>
    </form>
  </div>
  <p class="hint">
    Each project is a folder with <code>project.json</code> and <code>flows.db</code>: copy it to back it up or hand it over.
    Agents cannot switch projects; they always work in the current one{live.status ? ` (${live.status.project})` : ''}.
  </p>
</section>

<style>
  .projects {
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
    margin: 0 0 10px;
    font-size: 15px;
  }
  form {
    display: grid;
    gap: 10px;
  }
  label {
    display: grid;
    gap: 5px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-2);
  }
  em {
    font-style: normal;
    font-weight: 400;
    color: var(--text-3);
  }
  input,
  textarea {
    padding: 6px 10px;
    border: 1px solid var(--hairline);
    border-radius: 8px;
    background: var(--field);
    color: var(--text);
    font: 12.5px/1.5 var(--mono);
    outline: none;
  }
  input:focus,
  textarea:focus {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: var(--surface);
  }
  textarea {
    resize: vertical;
  }
  .row {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .row input {
    flex: 1;
  }
  .card > .row {
    margin-top: 12px;
  }
  button {
    border: 1px solid var(--hairline);
    border-radius: 8px;
    padding: 5px 14px;
    background: var(--surface);
    color: var(--text);
    white-space: nowrap;
  }
  button:hover:not(:disabled) {
    background: var(--hover);
  }
  button:disabled {
    opacity: 0.55;
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
  .danger {
    color: var(--red);
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .muted {
    font-size: 12px;
    color: var(--text-3);
  }
  .list {
    display: grid;
    gap: 8px;
  }
  article {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 10px 12px;
    border: 1px solid var(--hairline);
    border-radius: 10px;
  }
  article.loaded {
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: color-mix(in srgb, var(--accent) 5%, transparent);
  }
  .main {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 4px;
  }
  .title {
    font-weight: 600;
  }
  .path {
    font-size: 11.5px;
    color: var(--text-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .pill {
    padding: 0 8px;
    border-radius: 999px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-2);
    background: var(--field);
  }
  .pill.current {
    margin-left: 6px;
    color: var(--accent);
    background: var(--selected);
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .bad {
    margin: 0;
    color: var(--red);
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.5;
  }
</style>
