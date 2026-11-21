import { useEffect, useRef } from 'react';
import type { FileChangeMsg } from '@hubness/shared';
import { liveStream } from './stream';

/**
 * Live file changes on disk (agents, shells, other people): one SSE stream per workbench
 * (`useFileWatch`), fanned out in the page to the editors, the tree and git state
 * (`useFileChanges`). After a reconnect every watched project gets an `overflow` message —
 * anything may have changed while the stream was down.
 */

type Listener = (msg: FileChangeMsg) => void;
const listeners = new Set<Listener>();
const publish = (m: FileChangeMsg) => listeners.forEach((l) => l(m));

/** Open the change stream for these projects while the component is mounted. */
export function useFileWatch(projectIds: string[]) {
  const key = [...projectIds].sort().join(',');
  useEffect(() => {
    if (!key) return;
    const ids = key.split(',');
    let opened = false;
    return liveStream(
      `/api/watch?projects=${encodeURIComponent(key)}`,
      (data) => {
        try {
          publish(JSON.parse(data) as FileChangeMsg);
        } catch {}
      },
      {
        onOpen: () => {
          if (opened) for (const projectId of ids) publish({ projectId, paths: [], git: true, overflow: true });
          opened = true;
        },
      },
    );
  }, [key]);
}

/** React to changes on disk; the handler may change between renders. */
export function useFileChanges(fn: Listener) {
  const ref = useRef(fn);
  ref.current = fn;
  useEffect(() => {
    const l: Listener = (m) => ref.current(m);
    listeners.add(l);
    return () => void listeners.delete(l);
  }, []);
}

/** Does the message touch this file (or everything)? */
export const touches = (m: FileChangeMsg, projectId: string, path: string) =>
  m.projectId === projectId && (Boolean(m.overflow) || m.paths.includes(path) || m.paths.some((p) => path.startsWith(p + '/')));
