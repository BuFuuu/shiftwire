// Little text helpers for the right-click menus: copy, and the encodings pentesters
// convert by hand all day (URL, base64).

/** Copies to the clipboard (falls back to the old way where the API is not allowed). */
export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = Object.assign(document.createElement('textarea'), { value: text });
    document.body.append(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  }
}

/** The text selected on the page right now ('' if none). */
export const selectedText = () => window.getSelection()?.toString() ?? '';

export const urlEncode = (s: string) => encodeURIComponent(s);

export function urlDecode(s: string): string {
  try {
    return decodeURIComponent(s.replaceAll('+', ' '));
  } catch {
    return s; // not valid percent-encoding: leave it
  }
}

/** UTF-8 safe, unlike plain btoa. */
export function base64Encode(s: string): string {
  return btoa(Array.from(new TextEncoder().encode(s), (b) => String.fromCharCode(b)).join(''));
}

/** Accepts base64 and base64url, with or without padding. */
export function base64Decode(s: string): string {
  try {
    const clean = s.trim().replaceAll('-', '+').replaceAll('_', '/');
    const bytes = Uint8Array.from(atob(clean + '='.repeat((4 - (clean.length % 4)) % 4)), (c) => c.charCodeAt(0));
    return new TextDecoder().decode(bytes);
  } catch {
    return s;
  }
}
