<script lang="ts" module>
  import type { IconName } from './Icon.svelte';

  import type { View } from '../lib/nav.svelte';
  export type { View };

  export const items: { id: View; label: string; icon: IconName; key: string }[] = [
    { id: 'history', label: 'History', icon: 'history', key: '1' },
    { id: 'repeater', label: 'Repeater', icon: 'repeater', key: '2' },
    { id: 'sitemap', label: 'Site Map', icon: 'sitemap', key: '3' },
    { id: 'timeline', label: 'AI Timeline', icon: 'timeline', key: '4' },
  ];
</script>

<script lang="ts">
  import Icon from './Icon.svelte';
  import { live, timeline } from '../lib/live.svelte';

  let { view = $bindable() }: { view: View } = $props();
</script>

<nav class="sidebar">
  <div class="brand"><img src="/favicon.svg" alt="" width="20" height="20" />shiftWire</div>
  <button class="project" class:active={view === 'projects'} onclick={() => (view = 'projects')} title="Projects: switch or create ({live.status?.project_path ?? ''})">
    <Icon name="projects" />
    <span class="name">{live.status?.project ?? '…'}</span>
    <span class="chev">⌄</span>
  </button>
  <ul>
    {#each items as item (item.id)}
      <li>
        <button class:active={view === item.id} onclick={() => (view = item.id)} title="{item.label} (Alt+{item.key})">
          <span class="ic" style:color="var(--ic-{item.id})"><Icon name={item.icon} /></span> {item.label}
          {#if item.id === 'timeline' && timeline.unseenMarks > 0}
            <span class="badge" title="New marks from agents">{timeline.unseenMarks}</span>
          {/if}
        </button>
      </li>
    {/each}
  </ul>
  <div class="spacer"></div>
  <button class:active={view === 'settings'} onclick={() => (view = 'settings')}>
    <Icon name="settings" /> Settings
  </button>
  <footer>
    <span class="dot" class:on={live.connected}></span>
    {live.connected ? 'Live' : 'Reconnecting…'}
    <span class="muted">{live.status ? `v${live.status.version}` : ''}</span>
  </footer>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 14px 10px 10px;
    background: var(--sidebar);
    backdrop-filter: saturate(180%) blur(20px);
    border-right: 1px solid var(--hairline);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    letter-spacing: -0.01em;
    padding: 4px 10px 14px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 2px;
  }
  button {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 6px 10px;
    border: 0;
    border-radius: 7px;
    background: none;
    color: var(--text);
    text-align: left;
  }
  button:hover {
    background: var(--hover);
  }
  button.active {
    background: var(--selected);
    color: var(--accent);
    font-weight: 500;
  }
  .project {
    margin: -6px 0 10px;
    border: 1px solid var(--hairline);
    color: var(--text-2);
    font-size: 12.5px;
  }
  .project .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
    font-weight: 500;
  }
  .project .chev {
    color: var(--text-3);
  }
  .badge {
    margin-left: auto;
    min-width: 18px;
    padding: 0 5px;
    border-radius: 9px;
    background: var(--red);
    color: #fff;
    font-size: 11px;
    font-weight: 600;
    text-align: center;
  }
  .ic {
    display: inline-flex;
  }
  .spacer {
    flex: 1;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 10px 10px 2px;
    font-size: 12px;
    color: var(--text-2);
  }
  .muted {
    margin-left: auto;
    color: var(--text-3);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-3);
  }
  .dot.on {
    background: var(--green);
  }
</style>
