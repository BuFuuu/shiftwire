<script lang="ts" generics="T">
  // Renders only the rows in view (plus a margin), so 100k rows scroll like 100.
  import type { Snippet } from 'svelte';

  let {
    items,
    key,
    row,
    rowHeight = 26,
    viewport = $bindable(),
    onscroll,
  }: {
    items: T[];
    key: (item: T) => string;
    row: Snippet<[T, number]>;
    rowHeight?: number;
    viewport?: HTMLDivElement;
    onscroll?: () => void;
  } = $props();

  const OVERSCAN = 12;
  let scrollTop = $state(0);
  let height = $state(0);

  const start = $derived(Math.max(0, Math.floor(scrollTop / rowHeight) - OVERSCAN));
  const end = $derived(Math.min(items.length, Math.ceil((scrollTop + height) / rowHeight) + OVERSCAN));
  const visible = $derived(items.slice(start, end));

  function handleScroll() {
    scrollTop = viewport!.scrollTop;
    onscroll?.();
  }
</script>

<div class="viewport" bind:this={viewport} bind:clientHeight={height} onscroll={handleScroll}>
  <div class="spacer" style:height="{items.length * rowHeight}px">
    <div class="window" style:transform="translateY({start * rowHeight}px)">
      {#each visible as item, i (key(item))}
        {@render row(item, start + i)}
      {/each}
    </div>
  </div>
</div>

<style>
  .viewport {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    scrollbar-gutter: stable;
    outline: none;
  }
  .spacer {
    position: relative;
  }
  .window {
    will-change: transform;
  }
</style>
