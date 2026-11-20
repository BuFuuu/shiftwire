// Which view is open, shared so any part of the UI can navigate
// (e.g. "Send to Repeater" in History switches to the Repeater).

export type View = 'history' | 'timeline' | 'sitemap' | 'repeater' | 'projects' | 'settings';
export type SettingsSection = 'scope' | 'rules' | 'agents' | 'masking' | 'network' | 'certificate';

export const nav = $state({
  view: 'history' as View,
  /** The Repeater tab in front. */
  repeaterTab: null as string | null,
  /** The Settings section shown (Settings lists them on the left). */
  // `invisible` was a section until the invisible proxy was removed.
  settings: (localStorage.getItem('shiftwire.settings')?.replace(/^invisible$/, 'scope') ?? 'scope') as SettingsSection,
  /** The AI Timeline tab shows the plans (intents and their requests) or the raw MCP log. */
  aiTab: (localStorage.getItem('shiftwire.aiTab') ?? 'plans') as 'plans' | 'mcp',
});

export function setAiTab(tab: 'plans' | 'mcp'): void {
  nav.aiTab = tab;
  localStorage.setItem('shiftwire.aiTab', tab);
}

export function openSettings(section: SettingsSection): void {
  nav.view = 'settings';
  nav.settings = section;
  localStorage.setItem('shiftwire.settings', section);
}
