import { useEffect, useSyncExternalStore } from 'react';
import { useLocation } from 'react-router';

/**
 * Open places (projects, groups, chats) shown as tabs in the top bar, like browser tabs:
 * leaving a project keeps it one click away. Each place remembers its last URL, so coming
 * back restores the focused file / tool / session (the workbench layout itself is stored
 * per project). Kept in localStorage — a per-browser convenience.
 */

export type PlaceKind = 'project' | 'group' | 'chat';

export interface Place {
  key: string;
  kind: PlaceKind;
  title: string;
  /** Path that identifies the place (`/projects/<id>`). */
  path: string;
  /** Last full URL inside the place (path + query). */
  url: string;
}

const STORE = 'hub.places';
const MAX = 12;

let places: Place[] = (() => {
  try {
    const v = JSON.parse(localStorage.getItem(STORE) ?? '[]') as Place[];
    return Array.isArray(v) ? v.filter((p) => p && typeof p.key === 'string' && typeof p.url === 'string') : [];
  } catch {
    return [];
  }
})();
const listeners = new Set<() => void>();

function commit(next: Place[]) {
  places = next;
  try {
    localStorage.setItem(STORE, JSON.stringify(places));
  } catch {}
  listeners.forEach((l) => l());
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export const usePlaces = () => useSyncExternalStore(subscribe, () => places);

/** Add or refresh a place; new ones go to the end, the oldest fall off past MAX. */
export function touchPlace(p: Place) {
  const i = places.findIndex((x) => x.key === p.key);
  if (i >= 0) {
    const cur = places[i]!;
    if (cur.title === p.title && cur.url === p.url && cur.path === p.path) return;
    const next = [...places];
    next[i] = p;
    return commit(next);
  }
  commit([...places, p].slice(-MAX));
}

/** Close a place; returns the neighbour to go to when it was the current one. */
export function closePlace(key: string): Place | null {
  const i = places.findIndex((x) => x.key === key);
  if (i < 0) return null;
  const next = places.filter((x) => x.key !== key);
  commit(next);
  return next[i] ?? next[i - 1] ?? null;
}

/** Register the current page as a place and keep its URL fresh while it is open. */
export function usePlace(p: { key: string; kind: PlaceKind; title: string; path: string } | null) {
  const loc = useLocation();
  const url = loc.pathname + loc.search;
  useEffect(() => {
    if (!p || !p.title) return;
    // only while we are really inside the place (redirects pass through other paths)
    if (loc.pathname !== p.path && !loc.pathname.startsWith(`${p.path}/`)) return;
    touchPlace({ ...p, url });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p?.key, p?.title, p?.path, url]);
}
