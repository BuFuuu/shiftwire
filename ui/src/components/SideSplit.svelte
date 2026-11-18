<script lang="ts">
  // Left/right split with a draggable divider (request | response). The
  // position is remembered per `name`; double-click the divider to center it.
  import type { Snippet } from 'svelte';

  let { left, right, name }: { left: Snippet; right: Snippet; name: string } = $props();

  const key = $derived(`shiftwire.split.${name}`);
  let container: HTMLDivElement | undefined = $state();
  // svelte-ignore state_referenced_locally
  let ratio = $state(Number(localStorage.getItem(`shiftwire.split.${name}`)) || 0.5);

  function drag(e: PointerEvent) {
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const rect = container!.getBoundingClientRect();
      ratio = Math.min(0.85, Math.max(0.15, (ev.clientX - rect.left) / rect.width));
    };
    const up = () => {
      localStorage.setItem(key, String(ratio));
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
  }

  function reset() {
    ratio = 0.5;
    localStorage.setItem(key, '0.5');
  }
</script>

<div class="split" bind:this={container}>
  <div class="side" style:flex-basis="{ratio * 100}%">{@render left()}</div>
  <div
    class="handle"
    role="separator"
    aria-orientation="vertical"
    title="Drag to resize · double-click to center"
    onpointerdown={drag}
    ondblclick={reset}
  ></div>
  <div class="side" style:flex-basis="{(1 - ratio) * 100}%">{@render right()}</div>
</div>

<style>
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .side {
    min-width: 0;
    min-height: 0;
    flex-grow: 0;
    flex-shrink: 1;
    display: flex;
    flex-direction: column;
  }
  .handle {
    width: 7px;
    margin: 0 -3px;
    position: relative;
    z-index: 2;
    flex: none;
    cursor: col-resize;
  }
  .handle::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 3px;
    border-left: 1px solid var(--hairline);
  }
  .handle:hover::after {
    border-left: 2px solid var(--accent);
    left: 2.5px;
  }
</style>
