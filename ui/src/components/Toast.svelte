<script lang="ts">
  // "claude marked … as high": appears for a few seconds when an agent marks something.
  import { alerts, jumpTo, timeline } from '../lib/live.svelte';
  import { nav } from '../lib/nav.svelte';

  function showAlert() {
    if (!alerts.last) return;
    jumpTo(`id:${alerts.last.seq}`, `#${alerts.last.seq}`);
    alerts.last = null;
  }

  function open() {
    nav.view = 'timeline';
    timeline.toast = null;
  }
</script>

{#if alerts.last}
  {@const a = alerts.last}
  <button class="toast alert" onclick={showAlert} title="Show this request in History">
    <span class="sev">⚠ {a.rule}</span>
    <span class="text"><b>#{a.seq}</b> {a.method} {a.url}</span>
  </button>
{:else if timeline.toast}
  {@const t = timeline.toast}
  <button class="toast" onclick={open} title="Open the AI Timeline">
    <span class="sev sev-{t.severity}">★ {t.severity}</span>
    <span class="text"><b>{t.actor}</b> marked a request: {t.text}</span>
  </button>
{/if}

<style>
  .toast {
    position: fixed;
    right: 18px;
    bottom: 18px;
    z-index: 60;
    max-width: 420px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    border: 1px solid var(--hairline);
    border-radius: 12px;
    background: color-mix(in srgb, var(--surface) 92%, transparent);
    backdrop-filter: saturate(180%) blur(20px);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.2);
    color: var(--text);
    text-align: left;
    animation: slide 160ms ease-out;
  }
  @keyframes slide {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
  }
  .sev {
    flex: none;
    padding: 1px 7px;
    border-radius: 8px;
    font-size: 11px;
    font-weight: 600;
    color: #fff;
    background: var(--sev);
  }
  .alert .sev {
    background: var(--orange);
  }
  .text {
    font-size: 12.5px;
    line-height: 1.4;
  }
</style>
