import type { PickedElement } from './previewDrafts.ts';

/**
 * What the Preview panel accepts from the page inside its frame. The page is the app being
 * built — possibly broken, possibly hostile — on another origin; all it can do is post
 * messages. Each one is checked here (pure, tested) before the panel acts on it.
 */

export type FrameMessage =
  | { type: 'ready' }
  | { type: 'location'; path: string }
  | { type: 'pick'; element: PickedElement }
  | { type: 'pick-cancel' };

const MAX_PATH = 2000;

/**
 * A path inside the previewed app: starts with one `/`, so that origin + path can never be
 * another site (`//evil.example`, `/\evil.example`), and has no control characters.
 */
export function safePath(p: unknown): string | null {
  if (typeof p !== 'string' || p.length > MAX_PATH) return null;
  if (!p.startsWith('/') || p.startsWith('//') || p.startsWith('/\\')) return null;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(p)) return null;
  return p;
}

/** What the user typed into the address bar, as a path (`/` when it can't be one). */
export function typedPath(input: unknown): string {
  const s = typeof input === 'string' ? input.trim() : '';
  return safePath(s.startsWith('/') ? s : `/${s}`) ?? '/';
}

const str = (v: unknown, max: number) => (typeof v === 'string' ? v : '').slice(0, max);
const num = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) ? Math.max(0, Math.round(v)) : 0);
const box = (v: unknown) => {
  const o = (v && typeof v === 'object' ? v : {}) as Record<string, unknown>;
  return { width: num(o.width), height: num(o.height) };
};

/**
 * Check a `message` event. `fromFrame` — its source is our frame's window; `origin` must be
 * the preview's own; a pick counts only while the panel asked for one.
 */
export function parseFrameMessage(data: unknown, from: { fromFrame: boolean; origin: string; expectedOrigin: string; picking: boolean }): FrameMessage | null {
  if (!from.fromFrame || from.origin !== from.expectedOrigin) return null;
  if (!data || typeof data !== 'object') return null;
  const m = data as Record<string, unknown>;
  if (m.hubness !== 1) return null;
  switch (m.type) {
    case 'ready':
      return { type: 'ready' };
    case 'pick-cancel':
      return { type: 'pick-cancel' };
    case 'location': {
      const path = safePath(m.path);
      return path ? { type: 'location', path } : null;
    }
    case 'pick': {
      if (!from.picking) return null;
      return {
        type: 'pick',
        element: { selector: str(m.selector, 500), text: str(m.text, 300), html: str(m.html, 1500), rect: box(m.rect), viewport: box(m.viewport), path: safePath(m.path) ?? '/' },
      };
    }
    default:
      return null;
  }
}
