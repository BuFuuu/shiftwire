<script lang="ts" module>
  export type Tab = 'pretty' | 'raw';
  // Each pane remembers its own choice, across selections: picking Raw for the
  // response does not change the request.
  export const prefs = $state<Record<'request' | 'response', Tab>>({ request: 'pretty', response: 'pretty' });
</script>

<script lang="ts">
  import type { Message } from '../lib/types';
  import { bytes, headerValue, hexPreview } from '../lib/format';
  import { escapeHtml, highlightForm, highlightHead, highlightJson, highlightMarkup } from '../lib/highlight';

  let { title, pane, message }: { title: string; pane: 'request' | 'response'; message: Message | null } = $props();

  const SHOW_LIMIT = 200_000;
  let showAll = $state(false);
  $effect(() => {
    void message;
    showAll = false; // new message: start collapsed again
  });

  const tab = $derived(prefs[pane]);
  const mime = $derived(message ? (headerValue(message.head, 'content-type')?.split(';')[0].trim().toLowerCase() ?? '') : '');
  const body = $derived(message?.body);
  const isImage = $derived(!!body?.base64 && mime.startsWith('image/'));
  const text = $derived(body?.text ? (showAll ? body.text : body.text.slice(0, SHOW_LIMIT)) : '');
  const hidden = $derived(body?.text && !showAll ? Math.max(0, body.text.length - SHOW_LIMIT) : 0);

  /** Pretty: the same content as Raw, structured and colored. Nothing is left out. */
  const prettyBody = $derived.by(() => {
    if (!text) return '';
    const json = mime.includes('json') || /^\s*[{[]/.test(text) ? highlightJson(text) : null;
    if (json !== null) return json;
    if (/html|xml|svg/.test(mime) || /^\s*</.test(text)) return highlightMarkup(text);
    if (mime === 'application/x-www-form-urlencoded') return highlightForm(text);
    return escapeHtml(text);
  });
</script>

<section class="pane syntax">
  <header>
    <h3>{title}</h3>
    <div class="tabs" role="tablist" aria-label="{title} view">
      {#each ['pretty', 'raw'] as const as t (t)}
        <button role="tab" aria-selected={tab === t} class:on={tab === t} onclick={() => (prefs[pane] = t)}>
          {t === 'pretty' ? 'Pretty' : 'Raw'}
        </button>
      {/each}
    </div>
  </header>

  <div class="content">
    {#if !message}
      <p class="none">No response.</p>
    {:else}
      {#if tab === 'pretty'}
        <pre class="head">{@html highlightHead(message.head)}</pre>
      {:else}
        <pre class="head raw">{message.head}</pre>
      {/if}
      {#if isImage && tab === 'pretty'}
        <img src="data:{mime};base64,{body!.base64}" alt="response body" />
      {:else if body?.base64}
        <p class="note">Binary body, {bytes(body.size)}. First bytes:</p>
        <pre>{hexPreview(body.base64)}</pre>
      {:else if text}
        {#if tab === 'pretty'}<pre>{@html prettyBody}</pre>{:else}<pre class="raw">{text}</pre>{/if}
        {#if hidden > 0}
          <button class="more" onclick={() => (showAll = true)}>Show {bytes(hidden)} more</button>
        {/if}
      {:else}
        <p class="none">No body.</p>
      {/if}
    {/if}
  </div>

  {#if body && (body.decoded || body.truncated)}
    <footer>
      {#if body.decoded}Decoded from {body.decoded}.{/if}
      {#if body.truncated}Stored truncated; {bytes(body.size)} on the wire.{/if}
    </footer>
  {/if}
</section>

<style>
  .pane {
    flex: 1;
    min-width: 0;
    min-height: 0; /* scroll inside instead of growing past the window */
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 12px;
    border-bottom: 1px solid var(--hairline);
  }
  h3 {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .tabs {
    margin-left: auto;
    display: flex;
    padding: 2px;
    border-radius: 7px;
    background: var(--field);
  }
  .tabs button {
    border: 0;
    background: none;
    color: var(--text-2);
    font-size: 12px;
    padding: 2px 10px;
    border-radius: 5px;
  }
  .tabs button.on {
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
  }
  .content {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 10px 12px;
  }
  pre {
    margin: 0;
    font: 12px/1.55 var(--mono);
    white-space: pre-wrap;
    word-break: break-all;
  }
  pre.head {
    margin-bottom: 1.55em; /* one empty line, as on the wire */
  }
  pre.raw {
    color: var(--text);
  }
  img {
    max-width: 100%;
    border-radius: 6px;
    background: repeating-conic-gradient(var(--field) 0 25%, transparent 0 50%) 0 0 / 16px 16px;
  }
  .none,
  .note {
    margin: 0 0 8px;
    color: var(--text-3);
  }
  .more {
    margin-top: 8px;
    border: 1px solid var(--hairline);
    border-radius: 7px;
    background: var(--surface);
    color: var(--accent);
    padding: 3px 10px;
  }
  footer {
    padding: 5px 12px;
    border-top: 1px solid var(--hairline);
    font-size: 11.5px;
    color: var(--text-3);
  }
</style>
