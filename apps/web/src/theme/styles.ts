import type { Palette, Syntax, Theme } from './index';

/**
 * Palettes of the visual styles other than Strict (Strict uses THEMES from ./index). Each
 * style has its own palettes because its surfaces work differently: glass needs translucent
 * panels over a colourful backdrop, Expressive tonal containers instead of lines, Brutal
 * hard ink lines and loud flat accents.
 */

const SYNTAX_DARK: Syntax = { comment: '#8b949e', keyword: '#ff7b72', constant: '#79c0ff', entity: '#ffa657', function: '#d2a8ff', tag: '#7ee787', string: '#a5d6ff' };
const SYNTAX_LIGHT: Syntax = { comment: '#6e7781', keyword: '#cf222e', constant: '#0550ae', entity: '#953800', function: '#8250df', tag: '#116329', string: '#0a3069' };

/** Three soft colour blobs over a base: what the glass panels blur. */
const mesh = (a: string, b: string, c: string, base: string) =>
  `radial-gradient(60% 55% at 12% 8%, ${a}, transparent 62%), radial-gradient(55% 50% at 88% 18%, ${b}, transparent 62%), radial-gradient(70% 60% at 55% 105%, ${c}, transparent 65%), ${base}`;

function glass(id: string, label: string, d: { a: string; b: string; c: string; accent: string }, l: { a: string; b: string; c: string; accent: string }): Theme {
  return {
    id,
    label,
    dark: {
      backdrop: mesh(`${d.a}70`, `${d.b}60`, `${d.c}60`, '#0a0b12'),
      solid: '#161824',
      bg: 'rgba(10,11,18,0.30)',
      panel: 'rgba(255,255,255,0.075)',
      sunken: 'rgba(255,255,255,0.10)',
      border: 'rgba(255,255,255,0.15)',
      fg: '#f1f2f8',
      muted: '#a6a9bd',
      accent: d.accent,
      accentFg: '#0a0b12',
      ok: '#4ade80',
      warn: '#fbbf24',
      danger: '#f87171',
      syntax: SYNTAX_DARK,
    },
    light: {
      backdrop: mesh(`${l.a}90`, `${l.b}80`, `${l.c}80`, '#eef0f6'),
      solid: '#f7f8fc',
      bg: 'rgba(255,255,255,0.28)',
      panel: 'rgba(255,255,255,0.58)',
      sunken: 'rgba(20,24,48,0.06)',
      border: 'rgba(20,24,48,0.12)',
      fg: '#191b2c',
      muted: '#5a5e78',
      accent: l.accent,
      accentFg: '#ffffff',
      ok: '#15803d',
      warn: '#b45309',
      danger: '#dc2626',
      syntax: SYNTAX_LIGHT,
    },
  };
}

/** Material 3 Expressive-like tonal palettes: a seed hue carried through every surface. */
function tonal(id: string, label: string, dark: Omit<Palette, 'syntax' | 'ok' | 'warn' | 'danger'>, light: Omit<Palette, 'syntax' | 'ok' | 'warn' | 'danger'>): Theme {
  return {
    id,
    label,
    dark: { ...dark, ok: '#7fd9a0', warn: '#f2c063', danger: '#ffb4ab', syntax: SYNTAX_DARK },
    light: { ...light, ok: '#1e7a45', warn: '#8a5a00', danger: '#ba1a1a', syntax: SYNTAX_LIGHT },
  };
}

/** Neo-brutalist: paper and ink, one loud accent. */
function ink(id: string, label: string, accent: { dark: string; light: string; fg: string }): Theme {
  return {
    id,
    label,
    dark: {
      bg: '#121212',
      panel: '#1d1d1d',
      sunken: '#2a2a2a',
      border: '#f2f2f2',
      fg: '#f7f7f2',
      muted: '#b8b8b0',
      accent: accent.dark,
      accentFg: accent.fg,
      ok: '#4ade80',
      warn: '#fb923c',
      danger: '#f87171',
      syntax: SYNTAX_DARK,
    },
    light: {
      bg: '#fbf8ef',
      panel: '#ffffff',
      sunken: '#f1ede0',
      border: '#111111',
      fg: '#111111',
      muted: '#55524a',
      accent: accent.light,
      accentFg: accent.fg,
      ok: '#15803d',
      warn: '#c2410c',
      danger: '#dc2626',
      syntax: SYNTAX_LIGHT,
    },
  };
}

export const GLASS_THEMES: Theme[] = [
  glass('aurora', 'Aurora', { a: '#6d5dfc', b: '#22d3ee', c: '#c026d3', accent: '#9d8cff' }, { a: '#a5b4fc', b: '#67e8f9', c: '#f0abfc', accent: '#5b4cf0' }),
  glass('sunset', 'Sunset', { a: '#f97316', b: '#ec4899', c: '#8b5cf6', accent: '#ffa36c' }, { a: '#fdba74', b: '#f9a8d4', c: '#c4b5fd', accent: '#e8590c' }),
  glass('lagoon', 'Lagoon', { a: '#14b8a6', b: '#3b82f6', c: '#22c55e', accent: '#5eead4' }, { a: '#5eead4', b: '#93c5fd', c: '#86efac', accent: '#0f8f84' }),
  glass('graphite', 'Graphite', { a: '#64748b', b: '#94a3b8', c: '#475569', accent: '#e2e8f0' }, { a: '#cbd5e1', b: '#e2e8f0', c: '#94a3b8', accent: '#334155' }),
];

export const EXPRESSIVE_THEMES: Theme[] = [
  tonal(
    'lavender',
    'Lavender',
    { bg: '#151218', panel: '#211d26', sunken: '#2b2632', border: '#2b2632', fg: '#ece6f3', muted: '#aaa0b9', accent: '#cfbcff', accentFg: '#381e72' },
    { bg: '#fdf7ff', panel: '#f3edf7', sunken: '#e9e2ef', border: '#e9e2ef', fg: '#1d1b20', muted: '#625b71', accent: '#6750a4', accentFg: '#ffffff' },
  ),
  tonal(
    'mint',
    'Mint',
    { bg: '#0f1512', panel: '#1a221e', sunken: '#243029', border: '#243029', fg: '#dfe9e2', muted: '#99ab9f', accent: '#7fdcae', accentFg: '#00391f' },
    { bg: '#f5fbf5', panel: '#e9f1ea', sunken: '#dde8df', border: '#dde8df', fg: '#171d19', muted: '#55625a', accent: '#1e6b46', accentFg: '#ffffff' },
  ),
  tonal(
    'coral',
    'Coral',
    { bg: '#19120f', panel: '#261c18', sunken: '#332521', border: '#332521', fg: '#f1dfd8', muted: '#b9a097', accent: '#ffb59c', accentFg: '#5c1a04' },
    { bg: '#fff8f6', panel: '#fbeae4', sunken: '#f3ddd5', border: '#f3ddd5', fg: '#231915', muted: '#6f5a52', accent: '#9c4423', accentFg: '#ffffff' },
  ),
  tonal(
    'ocean',
    'Ocean',
    { bg: '#0f1418', panel: '#1a2127', sunken: '#232c34', border: '#232c34', fg: '#dee3ea', muted: '#98a3b0', accent: '#9ecaff', accentFg: '#003258' },
    { bg: '#f7f9ff', panel: '#eaf0f8', sunken: '#dde5ef', border: '#dde5ef', fg: '#171c22', muted: '#535f6b', accent: '#1f5f99', accentFg: '#ffffff' },
  ),
];

export const BRUTAL_THEMES: Theme[] = [
  ink('lemon', 'Lemon', { dark: '#ffd60a', light: '#ffd60a', fg: '#111111' }),
  ink('bubblegum', 'Bubblegum', { dark: '#ff6fb1', light: '#ff4fa0', fg: '#111111' }),
  ink('cobalt', 'Cobalt', { dark: '#5b8cff', light: '#2f5bff', fg: '#ffffff' }),
  ink('signal', 'Signal', { dark: '#ff4d3d', light: '#ff3b2f', fg: '#ffffff' }),
];
