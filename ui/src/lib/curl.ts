// "Copy as curl": a command that replays the captured request.

import { parseHeaders } from './format';
import type { FlowDetail } from './types';

const SKIP = new Set(['host', 'content-length', 'connection', 'proxy-connection', 'keep-alive', 'transfer-encoding']);

/** Single-quote for a POSIX shell: ' becomes '\'' */
function quote(s: string): string {
  return "'" + s.split("'").join("'\\''") + "'";
}

export function flowUrl(f: { scheme: string; host: string; port: number; path: string }): string {
  const standard = (f.scheme === 'https' && f.port === 443) || (f.scheme === 'http' && f.port === 80);
  return `${f.scheme}://${f.host}${standard ? '' : `:${f.port}`}${f.path}`;
}

export function toCurl(d: FlowDetail): string {
  const parts = [`curl -X ${d.method} ${quote(flowUrl(d))}`];
  for (const [name, value] of parseHeaders(d.request.head)) {
    if (!SKIP.has(name.toLowerCase())) parts.push(`-H ${quote(`${name}: ${value}`)}`);
  }
  const body = d.request.body;
  if (body.text) parts.push(`--data-binary ${quote(body.text)}`);
  else if (body.base64) parts.push(`--data-binary @<(echo ${quote(body.base64)} | base64 -d)`);
  return parts.join(' \\\n  ');
}
