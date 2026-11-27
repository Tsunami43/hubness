import { liveStream } from './stream';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { qk, useApi } from './query';
import type { GlobalStreamMsg, HubEvent, Pane, Session, SessionStreamMsg } from '@hubness/shared';
import { api } from './api';

/** Live session: snapshot + upsert of events from SSE. */
export function useSessionStream(sessionId: string | undefined) {
  const [session, setSession] = useState<Session | null>(null);
  const [events, setEvents] = useState<Map<string, HubEvent>>(new Map());
  const [connected, setConnected] = useState(false);
  const [pane, setPane] = useState<Pane | null>(null);

  useEffect(() => {
    if (!sessionId) return;
    setSession(null);
    setEvents(new Map());
    setPane(null);
    // the server sends a snapshot on every (re)connect, so a hub restart heals the view
    return liveStream(
      `/api/sessions/${sessionId}/stream`,
      (data) => {
      const msg = JSON.parse(data) as SessionStreamMsg;
      if (msg.type === 'snapshot') {
        setSession(msg.session);
        setEvents(new Map(msg.events.map((e) => [e.uid, e])));
        setPane(msg.pane ?? null);
      } else if (msg.type === 'pane') {
        setPane(msg.pane);
      } else if (msg.type === 'session') {
        setSession(msg.session);
      } else if (msg.type === 'event') {
        setEvents((prev) => new Map(prev).set(msg.event.uid, msg.event));
      }
      },
      { onOpen: () => setConnected(true), onDown: () => setConnected(false) },
    );
  }, [sessionId]);

  // sorted once per change of the map, not on every render of the view
  const list = useMemo(() => [...events.values()].sort((a, b) => a.seq - b.seq), [events]);
  return { session, setSession, events: list, connected, pane, setPane };
}

/** Scope of a session list: a project or a group. */
export type SessionScope = { projectId: string } | { groupId: string };

const scopeKey = (scope: SessionScope) => ('projectId' in scope ? (`p:${scope.projectId}` as const) : (`g:${scope.groupId}` as const));

/** Sessions of a project or group; kept live by useSessionSync. */
export function useSessions(scope: SessionScope) {
  const { data, error, loading, reload } = useApi(qk.sessions(scopeKey(scope)), () => api.sessions(scope));
  return { sessions: data, error, loading, reload };
}

/** All sessions (chats and projects) for the sidebar history; kept live by useSessionSync. */
export function useRecentSessions() {
  const { data, loading } = useApi(qk.sessions('all'), () => api.allSessions());
  return { sessions: data, loading };
}

/** The archive (every scope), most recently archived first; kept live by useSessionSync. */
export function useArchivedSessions() {
  const { data, loading } = useApi(qk.sessions('archived'), () => api.archivedSessions());
  return { sessions: data, loading };
}

const byUpdated = (a: Session, b: Session) => b.updatedAt.localeCompare(a.updatedAt);
const byArchived = (a: Session, b: Session) => (b.archivedAt ?? '').localeCompare(a.archivedAt ?? '');

/** Does a session belong to a cached list (`['sessions', scope]`)? */
function belongs(scope: string, s: Session): boolean {
  if (scope === 'archived') return Boolean(s.archivedAt);
  if (s.archivedAt) return false;
  if (scope === 'all') return true;
  if (scope.startsWith('p:')) return s.projectId === scope.slice(2);
  if (scope.startsWith('g:')) return s.groupId === scope.slice(2);
  return false;
}

/**
 * One connection to the global stream for the whole app: every cached session list is
 * updated in place (status, title, archive, deletion). Mounted once, in Layout.
 */
export function useSessionSync() {
  const client = useQueryClient();
  useEffect(() => {
    let first = true;
    return liveStream(
      '/api/stream',
      (data) => {
        const msg = JSON.parse(data) as GlobalStreamMsg;
        for (const [key] of client.getQueriesData<Session[]>({ queryKey: ['sessions'] })) {
          const scope = String(key[1]);
          client.setQueryData<Session[]>(key, (prev) => {
            if (!prev) return prev;
            if (msg.type === 'session-deleted') return prev.filter((s) => s.id !== msg.id);
            const rest = prev.filter((x) => x.id !== msg.session.id);
            if (!belongs(scope, msg.session)) return rest;
            return [msg.session, ...rest].sort(scope === 'archived' ? byArchived : byUpdated);
          });
        }
      },
      // reconnected after a hub restart: statuses may have changed meanwhile
      { onOpen: () => (first ? (first = false) : void client.invalidateQueries({ queryKey: ['sessions'] })) },
    );
  }, [client]);
}

/** Read a JSON value of localStorage (`initial` when missing or broken). */
function readStored<T>(key: string, initial: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : initial;
  } catch {
    return initial;
  }
}

export function useLocalStorage<T>(key: string, initial: T) {
  // the value belongs to its key: a component that lives on under another key (another
  // project in the same panel) must not show — or save — the previous key's value
  const [state, setState] = useState<{ key: string; value: T }>(() => ({ key, value: readStored(key, initial) }));
  const value = state.key === key ? state.value : readStored(key, initial);
  if (state.key !== key) setState({ key, value });
  const set = useCallback(
    (v: T) => {
      setState({ key, value: v });
      try {
        localStorage.setItem(key, JSON.stringify(v));
      } catch {}
    },
    [key],
  );
  return [value, set] as const;
}
