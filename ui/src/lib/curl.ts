// "Copy as curl": a command that replays the captured request.

import { parseHeaders } from './format';
import type { FlowDetail } from './types';

/** Backslash, newline, two spaces: one option per line in a shell. */
const JOIN = ' ' + String.fromCharCode(92) + String.fromCharCode(10) + '  ';

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
  const body = d.request.body;
  const data = body.text
    ? `--data-binary ${quote(body.text)}`
    : body.base64
      ? `--data-binary @<(echo ${quote(body.base64)} | base64 -d)`
      : '';
  return curlCommand(d.method, flowUrl(d), parseHeaders(d.request.head), data);
}

/** A request as written (method, URL, headers, body text), e.g. the Repeater's editor. */
export function curlOf(method: string, url: string, headers: [string, string][], body: string): string {
  return curlCommand(method, url, headers, body ? `--data-binary ${quote(body)}` : '');
}

function curlCommand(method: string, url: string, headers: [string, string][], data: string): string {
  const parts = [`curl -X ${method} ${quote(url)}`];
  for (const [name, value] of headers) {
    if (!SKIP.has(name.toLowerCase())) parts.push(`-H ${quote(`${name}: ${value}`)}`);
  }
  if (data) parts.push(data);
  return parts.join(JOIN);
}
