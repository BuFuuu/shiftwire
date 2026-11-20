<script lang="ts" generics="T">
  // Renders only the rows in view (plus a margin), so 100k rows scroll like 100.
  // `undefined` items are rows not loaded yet: an empty line of the same height,
  // and `onrange` says which rows are in view so the owner can load them.
  import type { Snippet } from 'svelte';

  let {
    items,
    key,
    row,
    rowHeight = 26,
    viewport = $bindable(),
    onscroll,
    onrange,
  }: {
    items: (T | undefined)[];
    key: (item: T) => string;
    row: Snippet<[T, number]>;
    rowHeight?: number;
    viewport?: HTMLDivElement;
    onscroll?: () => void;
    onrange?: (first: number, end: number) => void;
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

  // When the list gets shorter the browser moves the scroll position itself, and
  // not always with a scroll event: read it back after every change.
  $effect(() => {
    void items.length;
    if (viewport) scrollTop = viewport.scrollTop;
  });

  $effect(() => {
    void items;
    onrange?.(Math.floor(scrollTop / rowHeight), Math.ceil((scrollTop + height) / rowHeight));
  });
</script>

<div class="viewport" bind:this={viewport} bind:clientHeight={height} onscroll={handleScroll}>
  <div class="spacer" style:height="{items.length * rowHeight}px">
    <div class="window" style:transform="translateY({start * rowHeight}px)">
      {#each visible as item, i (item === undefined ? `\0${start + i}` : key(item))}
        {#if item === undefined}
          <div class="hole" style:height="{rowHeight}px"></div>
        {:else}
          {@render row(item, start + i)}
        {/if}
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
  .hole {
    background: repeating-linear-gradient(transparent 0 8px, var(--hairline) 8px 9px, transparent 9px 17px) 16px 0 / 40% 100% no-repeat;
    opacity: 0.5;
  }
</style>
