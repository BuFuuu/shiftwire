// Which view is open, shared so any part of the UI can navigate
// (e.g. "Send to Repeater" in History switches to the Repeater).

export type View = 'history' | 'timeline' | 'sitemap' | 'repeater' | 'projects' | 'settings';

export const nav = $state({
  view: 'history' as View,
  /** The Repeater tab in front. */
  repeaterTab: null as string | null,
});
