<script lang="ts">
  // Top/bottom split with a draggable hairline. The bottom pane is optional.
  import type { Snippet } from 'svelte';

  let { top, bottom }: { top: Snippet; bottom?: Snippet } = $props();

  const MIN = 120;
  let container: HTMLDivElement | undefined = $state();
  let bottomHeight = $state(Math.round(window.innerHeight * 0.45));

  function drag(e: PointerEvent) {
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const rect = container!.getBoundingClientRect();
      bottomHeight = Math.min(rect.height - MIN, Math.max(MIN, rect.bottom - ev.clientY));
    };
    const up = () => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
  }
</script>

<div class="split" bind:this={container}>
  <div class="top">{@render top()}</div>
  {#if bottom}
    <div class="handle" role="separator" aria-orientation="horizontal" onpointerdown={drag}></div>
    <div class="bottom" style:height="{bottomHeight}px">{@render bottom()}</div>
  {/if}
</div>

<style>
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .top {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .handle {
    height: 7px;
    margin: -3px 0;
    position: relative;
    z-index: 2;
    cursor: row-resize;
  }
  .handle::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    top: 3px;
    border-top: 1px solid var(--hairline);
  }
  .handle:hover::after {
    border-top-color: var(--accent);
  }
  .bottom {
    flex: none;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
