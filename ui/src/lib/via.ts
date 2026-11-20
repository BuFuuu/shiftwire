// Which way a request came in (FlowSummary.via), for History's Via column and the detail pane.

import type { FlowSummary } from './types';

type Flow = Pick<FlowSummary, 'via' | 'actor' | 'scheme' | 'host'>;

/** Short label and color per way in; unknown values show as they are. */
export const VIA: Record<string, { label: string; color: string }> = {
  proxy: { label: 'proxy', color: 'var(--text-2)' },
  browser: { label: 'browser', color: 'var(--accent)' },
  tunnel: { label: 'tunnel', color: 'var(--text-3)' },
  repeater: { label: 'repeater', color: 'var(--ic-repeater)' },
  mcp: { label: 'mcp', color: 'var(--ic-timeline)' },
  invisible: { label: 'invisible', color: 'var(--orange)' },
};

const isAgent = (actor: string) => actor !== 'proxy' && actor !== 'human';

/** One sentence: how this request reached shiftWire. */
export function describeVia(f: Flow): string {
  switch (f.via) {
    case 'proxy':
      return isAgent(f.actor)
        ? `Proxy port, sent by ${f.actor}'s own tool (its proxy credentials).`
        : 'Proxy port: a browser, VM or tool pointed at shiftWire.';
    case 'browser':
      return 'The AI browser shiftWire opened (status bar → AI browser).';
    case 'tunnel':
      return 'Tunnel through the proxy port, not decrypted (passthrough host or not TLS).';
    case 'repeater':
      return isAgent(f.actor) ? `Repeater, sent for ${f.actor}.` : 'Your Repeater.';
    case 'mcp':
      return `${f.actor} over MCP (send_request / repeat_flow).`;
    case 'invisible':
      return `Invisible proxy (removed in this version): a local address forwarded to ${f.scheme}://${f.host}.`;
    default:
      return f.via;
  }
}
