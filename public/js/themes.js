// Theme definitions and helpers. A theme is a set of CSS custom properties
// plus a "base" (light or dark) that sets the browser's color-scheme.

export const THEME_VARS = [
  { key: '--bg', label: 'App background' },
  { key: '--bg-sidebar', label: 'Sidebar' },
  { key: '--bg-list', label: 'Article list' },
  { key: '--bg-reader', label: 'Reader' },
  { key: '--surface', label: 'Inputs & cards' },
  { key: '--surface-hover', label: 'Hover' },
  { key: '--surface-active', label: 'Selected item' },
  { key: '--border', label: 'Borders' },
  { key: '--text', label: 'Text' },
  { key: '--text-muted', label: 'Secondary text' },
  { key: '--text-faint', label: 'Faint text' },
  { key: '--accent', label: 'Accent' },
  { key: '--accent-text', label: 'Text on accent' },
  { key: '--link', label: 'Links' },
  { key: '--unread', label: 'Unread marker' },
  { key: '--star', label: 'Star' },
  { key: '--danger', label: 'Danger' },
  { key: '--code-bg', label: 'Code background' },
];

export const BUILTIN_THEMES = {
  daylight: {
    name: 'Daylight', base: 'light',
    vars: {
      '--bg': '#f6f6f3', '--bg-sidebar': '#efefeb', '--bg-list': '#fafaf8', '--bg-reader': '#ffffff',
      '--surface': '#ffffff', '--surface-hover': '#e9e9e4', '--surface-active': '#e2e6f3', '--border': '#e2e2dc',
      '--text': '#1d1d1f', '--text-muted': '#5d5d63', '--text-faint': '#94949a',
      '--accent': '#3b5bdb', '--accent-text': '#ffffff', '--link': '#2f4fc4',
      '--unread': '#3b5bdb', '--star': '#e8a317', '--danger': '#d9480f', '--code-bg': '#f1f1ee',
    },
  },
  paper: {
    name: 'Paper', base: 'light',
    vars: {
      '--bg': '#f4ecd8', '--bg-sidebar': '#ede3cb', '--bg-list': '#f7f0de', '--bg-reader': '#fbf5e6',
      '--surface': '#fffaf0', '--surface-hover': '#e6dabd', '--surface-active': '#e8d7b0', '--border': '#e0d3b4',
      '--text': '#3b3127', '--text-muted': '#6e5f4d', '--text-faint': '#a08f77',
      '--accent': '#a0522d', '--accent-text': '#fffaf0', '--link': '#8b4513',
      '--unread': '#b5651d', '--star': '#c98a12', '--danger': '#b03a2e', '--code-bg': '#efe4c9',
    },
  },
  midnight: {
    name: 'Midnight', base: 'dark',
    vars: {
      '--bg': '#121316', '--bg-sidebar': '#16171b', '--bg-list': '#1a1b20', '--bg-reader': '#1e1f24',
      '--surface': '#25262c', '--surface-hover': '#2a2b32', '--surface-active': '#2e3550', '--border': '#2c2d34',
      '--text': '#e6e6e9', '--text-muted': '#a3a3ad', '--text-faint': '#6d6d78',
      '--accent': '#7c9cff', '--accent-text': '#0e1222', '--link': '#93adff',
      '--unread': '#7c9cff', '--star': '#f5c04e', '--danger': '#ff7a59', '--code-bg': '#26272e',
    },
  },
  nord: {
    name: 'Nord', base: 'dark',
    vars: {
      '--bg': '#2e3440', '--bg-sidebar': '#2b303b', '--bg-list': '#313744', '--bg-reader': '#343b48',
      '--surface': '#3b4252', '--surface-hover': '#3f4757', '--surface-active': '#434c5e', '--border': '#3b4252',
      '--text': '#eceff4', '--text-muted': '#c0c8d6', '--text-faint': '#8590a6',
      '--accent': '#88c0d0', '--accent-text': '#2e3440', '--link': '#8fbcbb',
      '--unread': '#88c0d0', '--star': '#ebcb8b', '--danger': '#bf616a', '--code-bg': '#3b4252',
    },
  },
  solarized: {
    name: 'Solarized', base: 'light',
    vars: {
      '--bg': '#eee8d5', '--bg-sidebar': '#e9e2cc', '--bg-list': '#f3eddb', '--bg-reader': '#fdf6e3',
      '--surface': '#fdf6e3', '--surface-hover': '#e4dcc3', '--surface-active': '#dfe3d0', '--border': '#ddd5bd',
      '--text': '#3c4a50', '--text-muted': '#657b83', '--text-faint': '#93a1a1',
      '--accent': '#268bd2', '--accent-text': '#fdf6e3', '--link': '#2176b3',
      '--unread': '#2aa198', '--star': '#b58900', '--danger': '#dc322f', '--code-bg': '#eee8d5',
    },
  },
  rosepine: {
    name: 'Rosé Pine', base: 'dark',
    vars: {
      '--bg': '#191724', '--bg-sidebar': '#1c1a29', '--bg-list': '#1f1d2e', '--bg-reader': '#232136',
      '--surface': '#26233a', '--surface-hover': '#2a273f', '--surface-active': '#393552', '--border': '#2f2c45',
      '--text': '#e0def4', '--text-muted': '#b4b0d0', '--text-faint': '#6e6a86',
      '--accent': '#ebbcba', '--accent-text': '#191724', '--link': '#c4a7e7',
      '--unread': '#eb6f92', '--star': '#f6c177', '--danger': '#eb6f92', '--code-bg': '#2a273f',
    },
  },
  forest: {
    name: 'Forest', base: 'dark',
    vars: {
      '--bg': '#141a16', '--bg-sidebar': '#161d18', '--bg-list': '#19211b', '--bg-reader': '#1c241e',
      '--surface': '#232d25', '--surface-hover': '#26302a', '--surface-active': '#2b3d30', '--border': '#28322b',
      '--text': '#e3eadf', '--text-muted': '#a7b5a3', '--text-faint': '#6b7a68',
      '--accent': '#8fcf8a', '--accent-text': '#122015', '--link': '#a6dba0',
      '--unread': '#8fcf8a', '--star': '#e6c35c', '--danger': '#e57c5e', '--code-bg': '#232d25',
    },
  },
};

export const FONTS = {
  serif: { label: 'Serif', css: '"Iowan Old Style", "Charter", "Bitstream Charter", "Sitka Text", Cambria, Georgia, serif' },
  sans: { label: 'Sans', css: 'system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif' },
  humanist: { label: 'Humanist', css: 'Seravek, "Gill Sans Nova", Ubuntu, Calibri, "DejaVu Sans", source-sans-pro, sans-serif' },
  mono: { label: 'Mono', css: 'ui-monospace, "Cascadia Code", "Source Code Pro", Menlo, Consolas, monospace' },
};

export const WIDTHS = { narrow: '34rem', medium: '42rem', wide: '52rem' };

const STORAGE_KEY = 'openrss.theme';

/** Resolves a theme reference ("auto", a built-in key or "custom:<id>") to a theme object. */
export function resolveTheme(ref, customThemes = []) {
  if (ref?.startsWith('custom:')) {
    const t = customThemes.find((c) => `custom:${c.id}` === ref);
    if (t) {
      const base = BUILTIN_THEMES[t.base === 'dark' ? 'midnight' : 'daylight'];
      return { name: t.name, base: t.base, vars: { ...base.vars, ...t.vars }, custom_css: t.custom_css || '' };
    }
  }
  if (BUILTIN_THEMES[ref]) return BUILTIN_THEMES[ref];
  const dark = window.matchMedia?.('(prefers-color-scheme: dark)').matches;
  return BUILTIN_THEMES[dark ? 'midnight' : 'daylight'];
}

/** Applies a theme and the reading preferences to the document. */
export function applyTheme(theme, prefs = {}) {
  const root = document.documentElement;
  for (const { key } of THEME_VARS) root.style.removeProperty(key);
  for (const [k, v] of Object.entries(theme.vars || {})) root.style.setProperty(k, v);
  root.dataset.base = theme.base === 'dark' ? 'dark' : 'light';
  root.style.colorScheme = root.dataset.base;

  const font = FONTS[prefs.font] || FONTS.serif;
  root.style.setProperty('--font-reading', font.css);
  root.style.setProperty('--reader-size', `${prefs.fontSize || 19}px`);
  root.style.setProperty('--reader-leading', String(prefs.lineHeight || 1.7));
  root.style.setProperty('--reader-width', WIDTHS[prefs.width] || WIDTHS.medium);
  root.dataset.density = prefs.density || 'comfortable';

  let style = document.getElementById('custom-theme-css');
  if (!style) {
    style = document.createElement('style');
    style.id = 'custom-theme-css';
    document.head.append(style);
  }
  style.textContent = theme.custom_css || '';

  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.content = theme.vars['--bg-sidebar'] || theme.vars['--bg'];

  // Cache for the pre-paint script in index.html to avoid a flash of the wrong theme.
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ base: root.dataset.base, vars: theme.vars, prefs }));
  } catch { /* storage unavailable */ }
}
