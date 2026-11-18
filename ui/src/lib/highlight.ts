// Tiny syntax highlighting for HTTP heads, JSON and HTML/XML.
//
// Everything that comes from a target is escaped first; only our own <span>s
// are added. Output is meant for {@html}.

const ESCAPES: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };

export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => ESCAPES[c]);
}

const span = (cls: string, text: string) => `<span class="${cls}">${escapeHtml(text)}</span>`;

/** Request line or status line, then `Name: value` headers. */
export function highlightHead(head: string): string {
  const [first = '', ...rest] = head.replace(/\r\n$/, '').split('\r\n');
  let line: string;
  const status = /^(HTTP\/[\d.]+)\s+(\d{3})(.*)$/.exec(first);
  if (status) {
    const cls = status[2][0] === '2' ? 'ok' : status[2][0] === '3' ? 'redirect' : status[2][0] === '4' ? 'client' : 'server';
    line = span('mut', status[1]) + ' ' + span(`code ${cls}`, status[2]) + escapeHtml(status[3]);
  } else {
    const [method = '', target = '', ...version] = first.split(' ');
    line = span('method', method) + ' ' + span('target', target) + ' ' + span('mut', version.join(' '));
  }
  const headers = rest.map((h) => {
    const i = h.indexOf(':');
    return i < 0 ? escapeHtml(h) : span('hk', h.slice(0, i)) + span('mut', ':') + span('hv', h.slice(i + 1));
  });
  return [line, ...headers].join('\n');
}

const JSON_TOKEN = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g;

/** Pretty-prints (if valid) and highlights JSON. Invalid JSON is shown as-is. */
export function highlightJson(text: string): string | null {
  let pretty: string;
  try {
    pretty = JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return null;
  }
  return jsonTokens(pretty);
}

const MARKUP_TOKEN = /<!--[\s\S]*?-->|<![^>]*>|<\/?[a-zA-Z][^>]*>/g;
const ATTR = /([^\s=/>]+)(\s*=\s*)("[^"]*"|'[^']*'|[^\s>]+)?/g;

/** Highlights tags, attributes and comments; the text is not reformatted. */
export function highlightMarkup(text: string): string {
  let out = '';
  let last = 0;
  for (const m of text.matchAll(MARKUP_TOKEN)) {
    out += escapeHtml(text.slice(last, m.index));
    out += m[0].startsWith('<!') ? span('com', m[0]) : highlightTag(m[0]);
    last = m.index! + m[0].length;
  }
  return out + escapeHtml(text.slice(last));
}

function highlightTag(tag: string): string {
  const name = /^<\/?([^\s/>]+)/.exec(tag)!;
  const end = /\/?>$/.exec(tag)?.[0] ?? '';
  const attrs = tag.slice(name[0].length, tag.length - end.length);
  let body = '';
  let last = 0;
  for (const m of attrs.matchAll(ATTR)) {
    body += escapeHtml(attrs.slice(last, m.index));
    body += span('attr', m[1]) + escapeHtml(m[2]) + (m[3] ? span('str', m[3]) : '');
    last = m.index! + m[0].length;
  }
  body += escapeHtml(attrs.slice(last));
  return span('tagp', name[0].slice(0, name[0].length - name[1].length)) + span('tag', name[1]) + body + span('tagp', end);
}

/** `a=1&b=2` with names and values told apart. */
export function highlightForm(text: string): string {
  return text
    .split('&')
    .map((pair) => {
      const i = pair.indexOf('=');
      return i < 0 ? span('attr', pair) : span('attr', pair.slice(0, i)) + span('mut', '=') + span('str', pair.slice(i + 1));
    })
    .join(span('mut', '&'));
}

/**
 * A request exactly as typed (the Repeater editor): colored but never
 * reformatted, so it lines up character for character with the textarea.
 */
export function highlightRequest(raw: string): string {
  const split = raw.search(/\r?\n\r?\n/);
  const head = split < 0 ? raw : raw.slice(0, split);
  const rest = split < 0 ? '' : raw.slice(split);
  const [first = '', ...headers] = head.split('\n');
  const line = /^(\S*)(\s*)(\S*)(.*)$/s.exec(first)!;
  const out = [span('method', line[1]) + escapeHtml(line[2]) + span('target', line[3]) + span('mut', line[4])];
  for (const h of headers) {
    const i = h.indexOf(':');
    out.push(i < 0 ? escapeHtml(h) : span('hk', h.slice(0, i)) + span('mut', ':') + span('hv', h.slice(i + 1)));
  }
  const gap = /^\r?\n\r?\n/.exec(rest)?.[0] ?? '';
  const body = rest.slice(gap.length);
  const type = /^content-type:(.*)$/im.exec(head)?.[1].toLowerCase() ?? '';
  let shown: string;
  if (type.includes('json') || /^\s*[{[]/.test(body)) shown = jsonTokens(body);
  else if (/html|xml/.test(type) || /^\s*</.test(body)) shown = highlightMarkup(body);
  else if (type.includes('x-www-form-urlencoded')) shown = highlightForm(body);
  else shown = escapeHtml(body);
  return out.join('\n') + escapeHtml(gap) + shown;
}

/** JSON colors on the text as it is (no pretty-printing; invalid JSON is fine). */
function jsonTokens(text: string): string {
  let out = '';
  let last = 0;
  for (const m of text.matchAll(JSON_TOKEN)) {
    out += escapeHtml(text.slice(last, m.index));
    if (m[1]) out += m[2] ? span('key', m[1]) + escapeHtml(m[2]) : span('str', m[1]);
    else if (m[3]) out += span('lit', m[3]);
    else out += span('num', m[0]);
    last = m.index! + m[0].length;
  }
  return out + escapeHtml(text.slice(last));
}
