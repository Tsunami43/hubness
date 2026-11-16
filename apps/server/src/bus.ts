import type { GlobalStreamMsg, SessionStreamMsg } from '@hubness/shared';

type Listener<T> = (msg: T) => void;

const sessionListeners = new Map<string, Set<Listener<SessionStreamMsg>>>();
const globalListeners = new Set<Listener<GlobalStreamMsg>>();

export const bus = {
  onSession(sessionId: string, fn: Listener<SessionStreamMsg>): () => void {
    let set = sessionListeners.get(sessionId);
    if (!set) sessionListeners.set(sessionId, (set = new Set()));
    set.add(fn);
    return () => {
      set.delete(fn);
      if (set.size === 0) sessionListeners.delete(sessionId);
    };
  },
  emitSession(sessionId: string, msg: SessionStreamMsg) {
    for (const fn of sessionListeners.get(sessionId) ?? []) fn(msg);
  },
  onGlobal(fn: Listener<GlobalStreamMsg>): () => void {
    globalListeners.add(fn);
    return () => globalListeners.delete(fn);
  },
  emitGlobal(msg: GlobalStreamMsg) {
    for (const fn of globalListeners) fn(msg);
  },
};
