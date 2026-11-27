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
  /**
   * Width (px) of an action revealed under the target: a release past half of it leaves the
   * target open by that much, so the action can be tapped (a list row's "Archive"); a tap on
   * the open target closes it. Without it a short swipe springs back.
   */
  open?: () => number;
  /**
   * While dragging, and when the target settles open or closed: progress 0…1 of the target
   * width, and whether release would commit.
   */
  onProgress?: (p: number, commit: boolean) => void;
  /**
   * The gesture committed: the target has slid out. `reset` brings it back (the action
   * failed, or the element stays on screen).
   */
  onCommit: (reset: () => void) => void;
}

interface Drag {
  id: number;
  /** Where the target was when the finger came down (open or closed). */
  base: number;
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
  /** The target's resting offset: 0, or `dir * open()` while its action is revealed. */
  const offset = useRef(0);

  const rest = (el: HTMLElement, dx: number) => {
    offset.current = dx;
    place(el, dx, true);
    o.current.onProgress?.(Math.abs(dx) / (el.offsetWidth || 1), false);
  };

  const settle = (commit: boolean, released = 0) => {
    const { target, dir, onCommit, open } = o.current;
    const el = target();
    drag.current = null;
    if (!el) return;
    if (commit) {
      offset.current = 0;
      place(el, dir * el.offsetWidth, true);
      setTimeout(() => onCommit(() => rest(el, 0)), 200);
      return;
    }
    const w = open?.() ?? 0;
    rest(el, w && released * dir > w / 2 ? dir * w : 0);
  };

  const willCommit = (d: Drag, width: number) => {
    const { dir, threshold = 0.35 } = o.current;
    const dist = d.dx * dir;
    return dist > width * threshold || (dist > 30 && d.v * dir > 0.5 && dist > Math.abs(d.base) + 30);
  };

  const bind = {
    style: { touchAction: 'pan-y' } as CSSProperties,
    onPointerDown(e: ReactPointerEvent) {
      if (e.pointerType !== 'touch' || !e.isPrimary) return;
      swiped.current = false;
      const base = offset.current;
      drag.current = { id: e.pointerId, base, x: e.clientX, y: e.clientY, lock: null, dx: base, lastX: e.clientX, lastT: e.timeStamp, v: 0 };
    },
    onPointerMove(e: ReactPointerEvent) {
      const d = drag.current;
      if (!d || e.pointerId !== d.id) return;
      const dx = e.clientX - d.x;
      const dy = e.clientY - d.y;
      if (!d.lock) {
        if (Math.abs(dx) < 8 && Math.abs(dy) < 8) return;
        // only a mostly horizontal move in our direction starts the swipe (or either way when open)
        d.lock = Math.abs(dx) > Math.abs(dy) * 1.2 && (d.base !== 0 || Math.sign(dx) === o.current.dir) ? 'x' : 'none';
        if (d.lock === 'x') {
          swiped.current = true;
          e.currentTarget.setPointerCapture?.(e.pointerId);
        }
      }
      if (d.lock !== 'x') return;
      // a swipe nested in another one (a row in the swipeable drawer) moves only itself
      e.stopPropagation();
      const el = o.current.target();
      if (!el) return;
      const dt = e.timeStamp - d.lastT;
      if (dt > 0) d.v = (e.clientX - d.lastX) / dt;
      d.lastX = e.clientX;
      d.lastT = e.timeStamp;
      d.dx = o.current.dir > 0 ? Math.max(0, d.base + dx) : Math.min(0, d.base + dx);
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
      e.stopPropagation();
      const el = o.current.target();
      // a finger that stopped before lifting is not a flick
      if (e.timeStamp - d.lastT > 80) d.v = 0;
      settle(Boolean(el) && willCommit(d, el!.offsetWidth), d.dx);
    },
    onPointerCancel(e: ReactPointerEvent) {
      const d = drag.current;
      if (!d || e.pointerId !== d.id) return;
      if (d.lock === 'x') settle(false, d.base);
      else drag.current = null;
    },
    onClickCapture(e: ReactMouseEvent) {
      const el = o.current.target();
      // the click that ends a swipe, or a tap on the open target (it closes): nothing opens
      if (swiped.current || (offset.current && el?.contains(e.target as Node))) {
        if (!swiped.current && el) rest(el, 0);
        swiped.current = false;
        e.preventDefault();
        e.stopPropagation();
      }
    },
  };

  return {
    /** Spread on the element that takes the gesture. */
    bind,
    /** Act as if swiped all the way (the revealed action was tapped). */
    commit: () => settle(true),
    /** Close an open target. */
    close: () => {
      const el = o.current.target();
      if (el && offset.current) rest(el, 0);
    },
  };
}
