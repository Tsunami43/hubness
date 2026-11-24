/**
 * Color themes. A theme sets base colors for the dark and light variants;
 * the other tokens (muted, sunken, diff colors…) are derived from them. Values
 * are written to CSS variables on <html> and read by Tailwind utilities (bg-bg, text-fg…).
 */

export interface Palette {
  bg: string; // workspace background
  fg: string; // primary text
  panel: string; // panels: navigation, cards, input field
  border: string; // borders
  /** Optional: otherwise derived from the base colors. */
  sunken?: string; // elements on a panel, hover
  muted?: string; // secondary text
  accentFg?: string; // text on accent fill
  accent: string; // accent: brand, active navigation, primary buttons
  ok: string; // "running / success"
  warn: string; // "waiting for reply", changes
  danger: string; // errors
  /** Code highlighting: roles as in the GitHub theme (lib/highlight.ts maps TextMate scopes to them). */
  syntax: Syntax;
}

export interface Syntax {
  comment: string;
  keyword: string; // keywords, storage
  constant: string; // constants, support, property names, markdown headings
  entity: string; // type/class names, variable definitions
  function: string; // function names
  tag: string; // HTML/JSX tags, JSON keys
  string: string; // strings, regexp, links
}

export interface Theme {
  id: string;
  label: string;
  dark: Palette;
  light: Palette;
}

export const THEMES: Theme[] = [
  {
    id: 'standard',
    label: 'Standard',
    dark: {
      bg: '#282c34',
      fg: '#9cdef2',
      panel: '#111111',
      border: '#355a66',
      accent: '#e06c75',
      ok: '#50fa7b',
      warn: '#f0ad4e',
      danger: '#ff5f66',
      // GitHub Dark palette; strings are warm: blue ones would blend with the theme's primary text
      syntax: { comment: '#8b949e', keyword: '#ff7b72', constant: '#79c0ff', entity: '#ffa657', function: '#d2a8ff', tag: '#7ee787', string: '#e5c07b' },
    },
    light: {
      bg: '#f3f5f7',
      fg: '#26313a',
      panel: '#ffffff',
      border: '#cbd3da',
      accent: '#b34d5c',
      ok: '#2e8b48',
      warn: '#b26a00',
      danger: '#c0392b',
      // GitHub Light palette
      syntax: { comment: '#6e7781', keyword: '#cf222e', constant: '#0550ae', entity: '#953800', function: '#8250df', tag: '#116329', string: '#0a3069' },
    },
  },
  {
    // Cendre (github.com/Aejkatappaja/cendre, hard variant): ash and embers — a warm
    // near-black background, ember accent. The original is dark-only; the light one is
    // ours, built on the same pigments (paper instead of ash).
    id: 'cendre',
    label: 'Cendre',
    dark: {
      bg: '#171311',
      fg: '#e6d5c2',
      panel: '#0f0c0a',
      sunken: '#201b19',
      border: '#362f2c',
      muted: '#a09384',
      accent: '#ea9875',
      accentFg: '#171311',
      ok: '#43b16a',
      warn: '#f4a21c',
      danger: '#d25780',
      syntax: { comment: '#73665b', keyword: '#d1766e', constant: '#99af6b', entity: '#4e89a2', function: '#fcba81', tag: '#ea9875', string: '#99af6b' },
    },
    light: {
      bg: '#f7f1ea',
      fg: '#2b211c',
      panel: '#fdfaf6',
      sunken: '#ede4da',
      border: '#d9cbbd',
      muted: '#7a6a5d',
      accent: '#b8582f',
      accentFg: '#ffffff',
      ok: '#2f8a4f',
      warn: '#b9770e',
      danger: '#b8405f',
      syntax: { comment: '#8f7f72', keyword: '#a8463f', constant: '#5f7a2e', entity: '#2f6a85', function: '#a8641d', tag: '#b8582f', string: '#5f7a2e' },
    },
  },
  {
    // Helix (the Helix editor's default theme): purple background, lavender text,
    // lilac accent. The light variant is ours; the original has none.
    id: 'helix',
    label: 'Helix',
    dark: {
      bg: '#3b224c',
      fg: '#a4a0e8',
      panel: '#281733',
      sunken: '#452859',
      border: '#5a5977',
      muted: '#8a86b0',
      accent: '#dbbfef',
      accentFg: '#281733',
      ok: '#9ff28f',
      warn: '#efba5d',
      danger: '#f47868',
      syntax: { comment: '#8a86b0', keyword: '#eccdba', constant: '#e8dca0', entity: '#dbbfef', function: '#ffffff', tag: '#f47868', string: '#cccccc' },
    },
    light: {
      bg: '#faf6fd',
      fg: '#3b224c',
      panel: '#ffffff',
      sunken: '#efe6f7',
      border: '#d8cce6',
      muted: '#7a6f8f',
      accent: '#6f44f0',
      accentFg: '#ffffff',
      ok: '#2f8a3a',
      warn: '#a56a00',
      danger: '#c8364e',
      syntax: { comment: '#7a6f8f', keyword: '#9a4f2c', constant: '#8a6d00', entity: '#6f44f0', function: '#3b224c', tag: '#c8364e', string: '#4d6a2f' },
    },
  },
  {
    id: 'github',
    label: 'GitHub',
    dark: {
      bg: '#0d1117',
      fg: '#e6edf3',
      panel: '#161b22',
      sunken: '#21262d',
      border: '#30363d',
      muted: '#7d8590',
      accent: '#2f81f7',
      accentFg: '#ffffff',
      ok: '#3fb950',
      warn: '#d29922',
      danger: '#f85149',
      syntax: { comment: '#8b949e', keyword: '#ff7b72', constant: '#79c0ff', entity: '#ffa657', function: '#d2a8ff', tag: '#7ee787', string: '#a5d6ff' },
    },
    light: {
      bg: '#ffffff',
      fg: '#1f2328',
      panel: '#f6f8fa',
      sunken: '#eaeef2',
      border: '#d0d7de',
      muted: '#656d76',
      accent: '#0969da',
      accentFg: '#ffffff',
      ok: '#1a7f37',
      warn: '#9a6700',
      danger: '#d1242f',
      syntax: { comment: '#6e7781', keyword: '#cf222e', constant: '#0550ae', entity: '#953800', function: '#8250df', tag: '#116329', string: '#0a3069' },
    },
  },
  {
    // Catppuccin: dark — Mocha, light — Latte
    id: 'catppuccin',
    label: 'Catppuccin',
    dark: {
      bg: '#1e1e2e',
      fg: '#cdd6f4',
      panel: '#181825',
      sunken: '#313244',
      border: '#45475a',
      muted: '#7f849c',
      accent: '#cba6f7',
      accentFg: '#1e1e2e',
      ok: '#a6e3a1',
      warn: '#fab387',
      danger: '#f38ba8',
      syntax: { comment: '#7f849c', keyword: '#cba6f7', constant: '#fab387', entity: '#f9e2af', function: '#89b4fa', tag: '#89dceb', string: '#a6e3a1' },
    },
    light: {
      bg: '#eff1f5',
      fg: '#4c4f69',
      panel: '#e6e9ef',
      sunken: '#dce0e8',
      border: '#bcc0cc',
      muted: '#8c8fa1',
      accent: '#8839ef',
      accentFg: '#ffffff',
      ok: '#40a02b',
      warn: '#df8e1d',
      danger: '#d20f39',
      syntax: { comment: '#8c8fa1', keyword: '#8839ef', constant: '#fe640b', entity: '#df8e1d', function: '#1e66f5', tag: '#04a5e5', string: '#40a02b' },
    },
  },
  {
    // Tokyo Night: dark — Night, light — Day
    id: 'tokyonight',
    label: 'Tokyo Night',
    dark: {
      bg: '#1a1b26',
      fg: '#c0caf5',
      panel: '#16161e',
      sunken: '#292e42',
      border: '#3b4261',
      muted: '#737aa2',
      accent: '#7aa2f7',
      accentFg: '#1a1b26',
      ok: '#9ece6a',
      warn: '#e0af68',
      danger: '#f7768e',
      syntax: { comment: '#565f89', keyword: '#bb9af7', constant: '#ff9e64', entity: '#2ac3de', function: '#7aa2f7', tag: '#f7768e', string: '#9ece6a' },
    },
    light: {
      bg: '#e1e2e7',
      fg: '#3760bf',
      panel: '#e9e9ed',
      sunken: '#d0d4e3',
      border: '#a8aecb',
      muted: '#6172b0',
      accent: '#2e7de9',
      accentFg: '#ffffff',
      ok: '#587539',
      warn: '#8c6c3e',
      danger: '#f52a65',
      syntax: { comment: '#848cb5', keyword: '#9854f1', constant: '#b15c00', entity: '#007197', function: '#2e7de9', tag: '#f52a65', string: '#587539' },
    },
  },
];

export type Mode = 'system' | 'dark' | 'light';
export const MODES: Mode[] = ['system', 'dark', 'light'];

// v2: the default theme changed to opencode — the previous Standard choice is not carried over
const THEME_KEY = 'hub.theme.v2';
const MODE_KEY = 'hub.mode';

const read = (k: string) => {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
};
const write = (k: string, v: string) => {
  try {
    localStorage.setItem(k, v);
  } catch {}
};

let themeId = THEMES.some((t) => t.id === read(THEME_KEY)) ? read(THEME_KEY)! : 'cendre';
let mode: Mode = MODES.includes(read(MODE_KEY) as Mode) ? (read(MODE_KEY) as Mode) : 'dark';

const media = window.matchMedia('(prefers-color-scheme: dark)');
const listeners = new Set<() => void>();

const mix = (a: string, b: string, pctA: number) => `color-mix(in srgb, ${a} ${pctA}%, ${b})`;

export const getThemeId = () => themeId;
export const getMode = () => mode;
export const isDark = () => (mode === 'system' ? media.matches : mode === 'dark');

export function applyTheme() {
  const theme = THEMES.find((t) => t.id === themeId) ?? THEMES[0];
  const dark = isDark();
  const p = dark ? theme.dark : theme.light;
  const vars: Record<string, string> = {
    '--color-bg': p.bg,
    '--color-fg': p.fg,
    '--color-surface': p.panel,
    '--color-line': p.border,
    // dark: between background and panel; light: slightly darker than background
    '--color-sunken': p.sunken ?? (dark ? mix(p.bg, p.panel, 55) : mix(p.fg, p.bg, 8)),
    '--color-muted': p.muted ?? mix(p.fg, p.bg, dark ? 58 : 62),
    '--color-accent': p.accent,
    '--color-accent-fg': p.accentFg ?? '#ffffff',
    '--color-ok': p.ok,
    '--color-warn': p.warn,
    '--color-danger': p.danger,
    '--color-add': mix(p.ok, 'transparent', dark ? 14 : 16),
    '--color-add-fg': dark ? mix(p.ok, p.fg, 70) : p.ok,
    '--color-del': mix(p.danger, 'transparent', dark ? 16 : 13),
    '--color-del-fg': dark ? mix(p.danger, p.fg, 72) : p.danger,
    ...Object.fromEntries(Object.entries(p.syntax).map(([k, v]) => [`--syntax-${k}`, v])),
  };
  const root = document.documentElement;
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
  root.style.colorScheme = dark ? 'dark' : 'light';
  root.dataset.theme = theme.id;
  document.querySelectorAll('meta[name="theme-color"]').forEach((m) => m.setAttribute('content', p.bg));
  listeners.forEach((fn) => fn());
}

export function setTheme(id: string) {
  themeId = id;
  write(THEME_KEY, id);
  applyTheme();
}

export function setMode(m: Mode) {
  mode = m;
  write(MODE_KEY, m);
  applyTheme();
}

/** Subscribe to theme changes (for components that show the current choice). */
export function onThemeChange(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

media.addEventListener('change', () => mode === 'system' && applyTheme());
applyTheme();
