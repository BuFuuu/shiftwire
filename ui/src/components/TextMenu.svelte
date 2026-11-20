<script lang="ts" module>
  /** A menu line: an action, or `null` for a separator. */
  export type MenuItem = { label: string; action: () => unknown; disabled?: boolean; hint?: string } | null;
</script>

<script lang="ts">
  // A small right-click menu for text: request and response views, the Repeater
  // editor, the MCP log. Each place passes the items that make sense there.
  import { untrack } from 'svelte';

  let { x, y, items, onclose }: { x: number; y: number; items: MenuItem[]; onclose: () => void } = $props();

  let menu: HTMLDivElement | undefined = $state();
  let pos = $state(untrack(() => ({ left: x, top: y })));

  // Keep the menu on screen.
  $effect(() => {
    if (!menu) return;
    const r = menu.getBoundingClientRect();
    pos = { left: Math.min(x, innerWidth - r.width - 8), top: Math.min(y, innerHeight - r.height - 8) };
    menu.focus();
  });

  async function run(item: Exclude<MenuItem, null>) {
    onclose();
    await item.action();
  }

  function onWindowPointer(e: PointerEvent) {
    if (menu && !menu.contains(e.target as Node)) onclose();
  }
</script>

<svelte:window onpointerdown={onWindowPointer} onblur={onclose} />

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div class="menu" bind:this={menu} role="menu" tabindex="-1" style:left="{pos.left}px" style:top="{pos.top}px"
  onkeydown={(e) => e.key === 'Escape' && onclose()}>
  {#each items as item, i (i)}
    {#if item}
      <button class="item" role="menuitem" disabled={item.disabled} onclick={() => run(item)}>
        {item.label}{#if item.hint}<kbd>{item.hint}</kbd>{/if}
      </button>
    {:else if i > 0 && items[i - 1]}
      <div class="sep"></div>
    {/if}
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 50;
    min-width: 220px;
    padding: 6px;
    border-radius: 10px;
    border: 1px solid var(--hairline);
    background: color-mix(in srgb, var(--surface) 88%, transparent);
    backdrop-filter: saturate(180%) blur(24px);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.18);
    outline: none;
    animation: appear 90ms ease-out;
  }
  @keyframes appear {
    from {
      opacity: 0;
      transform: scale(0.98);
    }
  }
  .item {
    display: flex;
    align-items: center;
    gap: 16px;
    width: 100%;
    padding: 5px 8px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text);
    text-align: left;
    font-size: 13px;
    white-space: nowrap;
  }
  .item:hover:not(:disabled) {
    background: var(--accent);
    color: #fff;
  }
  .item:disabled {
    color: var(--text-3);
  }
  .sep {
    height: 1px;
    margin: 5px 4px;
    background: var(--hairline);
  }
  kbd {
    margin-left: auto;
    font: 11px var(--font);
    color: var(--text-3);
  }
  .item:hover kbd {
    color: inherit;
  }
</style>
