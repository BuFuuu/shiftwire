<script lang="ts">
  import Sidebar, { items, type View } from './components/Sidebar.svelte';
  import Repeater from './views/Repeater.svelte';
  import Settings from './views/Settings.svelte';
  import Projects from './views/Projects.svelte';
  import SiteMap from './views/SiteMap.svelte';
  import Timeline from './views/Timeline.svelte';
  import Toast from './components/Toast.svelte';
  import { nav } from './lib/nav.svelte';
  import Toolbar from './components/Toolbar.svelte';
  import History from './views/History.svelte';


  const titles: Record<View, string> = {
    history: 'History',
    timeline: 'AI Timeline',
    sitemap: 'Site Map',
    repeater: 'Repeater',
    projects: 'Projects',
    settings: 'Settings',
  };


  function onKey(e: KeyboardEvent) {
    const item = e.altKey && items.find((i) => i.key === e.key);
    if (item) {
      nav.view = item.id;
      e.preventDefault();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="shell">
  <Sidebar bind:view={nav.view} />
  <main>
    <Toolbar title={titles[nav.view]} filterable={nav.view === 'history'} />
    <div class="content">
      {#if nav.view === 'history'}
        <History />
      {:else if nav.view === 'repeater'}
        <Repeater />
      {:else if nav.view === 'projects'}
        <Projects />
      {:else if nav.view === 'settings'}
        <Settings />
      {:else if nav.view === 'timeline'}
        <Timeline />
      {:else if nav.view === 'sitemap'}
        <SiteMap />
      {/if}
    </div>
  </main>
</div>
<Toast />

<style>
  .shell {
    display: grid;
    grid-template-columns: 210px 1fr;
    grid-template-rows: minmax(0, 1fr); /* don't grow with content; panes scroll */
    height: 100vh;
  }
  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    background: var(--surface);
  }
  .content {
    flex: 1;
    min-height: 0;
    display: flex; /* views fill it and manage their own scrolling */
    flex-direction: column;
  }
</style>
