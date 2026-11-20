// Small display helpers.

export function time(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour12: false });
}

/** DD.MM.YYYY */
export function date(ms: number): string {
  const d = new Date(ms);
  const two = (n: number) => String(n).padStart(2, '0');
  return `${two(d.getDate())}.${two(d.getMonth() + 1)}.${d.getFullYear()}`;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** CSS class for a status code: ok / redirect / client / server. */
export function statusClass(status: number | null): string {
  if (status === null) return 'server';
  if (status < 300) return 'ok';
  if (status < 400) return 'redirect';
  if (status < 500) return 'client';
  return 'server';
}

/** `Name: value` lines after the first line of a stored head. */
export function parseHeaders(head: string): [string, string][] {
  return head
    .split('\r\n')
    .slice(1)
    .map((line) => line.split(/:(.*)/s))
    .filter((parts) => parts.length > 1 && parts[0].trim())
    .map(([name, value]) => [name.trim(), value.trim()]);
}

export function headerValue(head: string, name: string): string | null {
  const wanted = name.toLowerCase();
  return parseHeaders(head).find(([n]) => n.toLowerCase() === wanted)?.[1] ?? null;
}

/** Pretty-prints JSON; anything else is returned unchanged. */
export function pretty(text: string, mime: string | null): string {
  const looksJson = mime?.includes('json') || /^\s*[{[]/.test(text);
  if (!looksJson) return text;
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

/** Hex dump of the first bytes of a base64 body. */
export function hexPreview(base64: string, max = 512): string {
  const bin = atob(base64.slice(0, Math.ceil(max / 3) * 4)).slice(0, max); // whole 4-char groups
  const lines: string[] = [];
  for (let i = 0; i < bin.length; i += 16) {
    const chunk = [...bin.slice(i, i + 16)];
    const hex = chunk.map((c) => c.charCodeAt(0).toString(16).padStart(2, '0')).join(' ');
    const ascii = chunk.map((c) => (c >= ' ' && c <= '~' ? c : '.')).join('');
    lines.push(`${i.toString(16).padStart(6, '0')}  ${hex.padEnd(47)}  ${ascii}`);
  }
  return lines.join('\n');
}
