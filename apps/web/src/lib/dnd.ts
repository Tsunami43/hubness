import { useSyncExternalStore } from 'react';
import type { TabRef } from './layout';

/**
 * What is being dragged inside the workbench: a tab (from a group) or a file from the tree.
 * dataTransfer can't be read during dragover, so the payload lives here; the store also tells
 * the editor groups to show their drop zones.
 */
export type Drag =
  | { kind: 'tab'; from: string; key: string }
  | { kind: 'ref'; ref: TabRef }
  /** A folder from the tree: it can only be moved inside the tree. */
  | { kind: 'dir'; repo: string; path: string };

let current: Drag | null = null;
const listeners = new Set<() => void>();

export function startDrag(d: Drag) {
  current = d;
  // after the browser took the drag image — otherwise the drop zones steal the drag start
  setTimeout(() => listeners.forEach((l) => l()));
}

export function endDrag() {
  if (!current) return;
  current = null;
  listeners.forEach((l) => l());
}

export const getDrag = () => current;

export const useDragging = () =>
  useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => void listeners.delete(l);
    },
    () => current !== null,
  );
