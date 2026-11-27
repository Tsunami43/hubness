import { useRef, type CSSProperties, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from 'react';

/**
 * Horizontal swipe gestures for touch screens, like native apps: swipe a drawer away, swipe a
 * list row to act on it. The moving element follows the finger (styles are set directly on
 * the DOM node, no re-render per frame); on release the gesture commits when it went far
 * enough or was flicked, otherwise it springs back. Mouse and pen are ignored, vertical
 * scrolling keeps working (`touch-action: pan-y`), and the click that ends a swipe is
 * swallowed so a swiped link does not open.
 */

const EASE = 'transform 220ms cubic-bezier(0.32, 0.72, 0, 1)';

function place(el: HTMLElement, dx: number, animate: boolean) {
  el.style.transition = animate ? EASE : 'none';
  el.style.transform = dx ? `translateX(${dx}px)` : '';
}

/** Slide an element off-screen horizontally, then call `done` (a drawer closing by tap). */
export function slideOut(el: HTMLElement | null, dir: 1 | -1, done: () => void) {
  if (!el || matchMedia('(prefers-reduced-motion: reduce)').matches) return done();
  place(el, dir * el.offsetWidth, true);
  setTimeout(done, 200);
}

export interface SwipeOptions {
  /** 1: the gesture drags to the right, -1: to the left. */
  dir: 1 | -1;
  /** The element that moves with the finger. */
  target: () => HTMLElement | null;
  /** Share of the target width that commits on release (default 0.35). */
  threshold?: number;
  /** While dragging: progress 0…1 of the target width, and whether release would commit. */
  onProgress?: (p: number, commit: boolean) => void;
  /**
   * The gesture committed: the target has slid out. `reset` brings it back (the action
   * failed, or the element stays on screen).
   */
  onCommit: (reset: () => void) => void;
}

interface Drag {
  id: number;
  x: number;
  y: number;
  lock: 'x' | 'none' | null;
  dx: number;
  /** Last two samples for the flick velocity. */
  lastX: number;
  lastT: number;
  v: number;
}

export function useSwipe(opts: SwipeOptions) {
  const o = useRef(opts);
  o.current = opts;
  const drag = useRef<Drag | null>(null);
  const swiped = useRef(false);

  const settle = (commit: boolean) => {
    const { target, dir, onProgress, onCommit } = o.current;
    const el = target();
    drag.current = null;
    if (!el) return;
    if (commit) {
      place(el, dir * el.offsetWidth, true);
      setTimeout(() => onCommit(() => place(el, 0, true)), 200);
    } else {
      place(el, 0, true);
      onProgress?.(0, false);
    }
  };

  const willCommit = (d: Drag, width: number) => {
    const { dir, threshold = 0.35 } = o.current;
    const dist = d.dx * dir;
    return dist > width * threshold || (dist > 30 && d.v * dir > 0.5);
  };

  return {
    style: { touchAction: 'pan-y' } as CSSProperties,
    onPointerDown(e: ReactPointerEvent) {
      if (e.pointerType !== 'touch' || !e.isPrimary) return;
      swiped.current = false;
      drag.current = { id: e.pointerId, x: e.clientX, y: e.clientY, lock: null, dx: 0, lastX: e.clientX, lastT: e.timeStamp, v: 0 };
    },
    onPointerMove(e: ReactPointerEvent) {
      const d = drag.current;
      if (!d || e.pointerId !== d.id) return;
      const dx = e.clientX - d.x;
      const dy = e.clientY - d.y;
      if (!d.lock) {
        if (Math.abs(dx) < 8 && Math.abs(dy) < 8) return;
        // only a mostly horizontal move in our direction starts the swipe
        d.lock = Math.abs(dx) > Math.abs(dy) * 1.2 && Math.sign(dx) === o.current.dir ? 'x' : 'none';
        if (d.lock === 'x') {
          swiped.current = true;
          e.currentTarget.setPointerCapture?.(e.pointerId);
        }
      }
      if (d.lock !== 'x') return;
      const el = o.current.target();
      if (!el) return;
      const dt = e.timeStamp - d.lastT;
      if (dt > 0) d.v = (e.clientX - d.lastX) / dt;
      d.lastX = e.clientX;
      d.lastT = e.timeStamp;
      d.dx = o.current.dir > 0 ? Math.max(0, dx) : Math.min(0, dx);
      place(el, d.dx, false);
      o.current.onProgress?.(Math.abs(d.dx) / el.offsetWidth, willCommit(d, el.offsetWidth));
    },
    onPointerUp(e: ReactPointerEvent) {
      const d = drag.current;
      if (!d || e.pointerId !== d.id) return;
      if (d.lock !== 'x') {
        drag.current = null;
        return;
      }
      const el = o.current.target();
      // a finger that stopped before lifting is not a flick
      if (e.timeStamp - d.lastT > 80) d.v = 0;
      settle(Boolean(el) && willCommit(d, el!.offsetWidth));
    },
    onPointerCancel(e: ReactPointerEvent) {
      const d = drag.current;
      if (!d || e.pointerId !== d.id) return;
      if (d.lock === 'x') settle(false);
      else drag.current = null;
    },
    onClickCapture(e: ReactMouseEvent) {
      if (!swiped.current) return;
      swiped.current = false;
      e.preventDefault();
      e.stopPropagation();
    },
  };
}
