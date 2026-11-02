/** 24 px doodle icons: stroke paths drawn with round caps at a 1.9 px stroke. */
export const ICONS = {
  home: '<path d="M4 11.5 12 4.5l8 7V20H4z"/><path d="M10 20v-5h4v5"/>',
  captions: '<rect x="3" y="5.5" width="18" height="13" rx="3.5"/><path d="M7 11h5M14 11h3M7 14.5h3M12 14.5h5"/>',
  listening: '<path d="M4 12h1.5M8 8v8M11.5 5v14M15 9v6M18.5 11v2"/>',
  translation: '<path d="M4 6h8M8 4v2M5.5 6c.8 3 3 5.5 6 6.5M10.5 6c-.6 3-2.6 5.5-5.5 7"/><path d="M13 20l3.5-8 3.5 8M14.3 17.2h4.4"/>',
  screenshot: '<path d="M4 9V5.5A1.5 1.5 0 0 1 5.5 4H9M15 4h3.5A1.5 1.5 0 0 1 20 5.5V9M20 15v3.5a1.5 1.5 0 0 1-1.5 1.5H15M9 20H5.5A1.5 1.5 0 0 1 4 18.5V15"/>',
  shortcuts: '<rect x="3" y="7" width="18" height="11" rx="3"/><path d="M7 11h.01M10.5 11h.01M14 11h.01M17 11h.01M8 14.5h8"/>',
  history: '<path d="M6 4h10.5A1.5 1.5 0 0 1 18 5.5v14a.5.5 0 0 1-.5.5H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z"/><path d="M8 8.5h6M8 12h6M8 15.5h4"/>',
  about: '<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5M12 8h.01"/>',
  online: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.5 2.5 3.5 5.5 3.5 8.5s-1 6-3.5 8.5c-2.5-2.5-3.5-5.5-3.5-8.5s1-6 3.5-8.5z"/>',
  pc: '<rect x="3.5" y="4.5" width="17" height="11" rx="2"/><path d="M9 20h6M12 15.5V20"/>',
  server: '<rect x="4" y="4" width="16" height="7" rx="2"/><rect x="4" y="13" width="16" height="7" rx="2"/><path d="M8 7.5h.01M8 16.5h.01"/>',
  apps: '<rect x="4" y="4" width="7" height="7" rx="2"/><rect x="13" y="4" width="7" height="7" rx="2"/><rect x="4" y="13" width="7" height="7" rx="2"/><rect x="13" y="13" width="7" height="7" rx="2"/>',
  shield: '<path d="M12 3.5 19 6v5.5c0 4.2-3 7.6-7 9-4-1.4-7-4.8-7-9V6z"/><path d="m9 12 2 2 4-4"/>',
  info: '<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5M12 8h.01"/>',
  check: '<path d="m5 12.5 4.5 4.5L19 7.5"/>',
  alert: '<path d="M12 4 21 19.5H3z"/><path d="M12 10v4.5M12 17h.01"/>',
} as const;

export type IconName = keyof typeof ICONS;
