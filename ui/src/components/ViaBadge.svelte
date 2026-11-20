<script lang="ts">
  // Which way a request came in: proxy port, AI browser, tunnel, Repeater, an agent over MCP.
  import { VIA, describeVia } from '../lib/via';
  import type { FlowSummary } from '../lib/types';

  let { flow }: { flow: Pick<FlowSummary, 'via' | 'actor' | 'scheme' | 'host'> } = $props();
  const v = $derived(VIA[flow.via] ?? { label: flow.via, color: 'var(--text-2)' });
</script>

<span class="via" style:--v={v.color} title={describeVia(flow)}>{v.label}</span>

<style>
  .via {
    display: inline-flex;
    align-items: center;
    height: 16px;
    padding: 0 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--v) 14%, transparent);
    color: var(--v);
    font: 500 10.5px var(--mono);
    vertical-align: 1px;
    white-space: nowrap;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
