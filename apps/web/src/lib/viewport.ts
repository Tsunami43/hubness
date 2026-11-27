/**
 * Native-app viewport on touch devices.
 *
 * iOS Safari never resizes the page for the on-screen keyboard: the layout keeps its full
 * height, the keyboard covers its bottom and the browser scrolls the whole document to
 * reveal the focused field — headers jump, the bottom navigation rides on top of the
 * keyboard, and fields below 16px zoom the page in. Here the app is pinned to the *visible*
 * area instead:
 *
 * - `--app-top` / `--app-h` on <html> follow `visualViewport`; `#root` and full-screen
 *   overlays are sized by them (index.css), so the screen shrinks above the keyboard and
 *   nothing scrolls under it;
 * - `data-kb` on <html> while the keyboard is open: the bottom navigation hides and the
 *   home-indicator padding (`--safe-b`) drops to 0;
 * - pinch zoom is blocked (iOS ignores `user-scalable=no`); double-tap zoom is off via
 *   `touch-action: manipulation` in CSS.
 *
 * On desktop nothing is tracked: trackpad pinch zoom must not shrink the app.
 */

const KEYBOARD_MIN = 120;

const isEditable = (el: Element | null): boolean => {
  if (!el) return false;
  if (el instanceof HTMLTextAreaElement) return !el.readOnly;
  if (el instanceof HTMLInputElement) return !el.readOnly && !/^(button|checkbox|color|file|hidden|image|radio|range|reset|submit)$/.test(el.type);
  return el instanceof HTMLElement && el.isContentEditable;
};

/**
 * Scroll the focused field's nearest scrolling container (a sheet's body, a page) so the field
 * is fully visible, with a little room around it. Only that container moves — never the
 * document, which iOS would shift as a whole.
 */
function revealInScroller(el: Element | null) {
  if (!(el instanceof HTMLElement)) return;
  let box = el.parentElement;
  while (box && box !== document.body) {
    const oy = getComputedStyle(box).overflowY;
    if ((oy === 'auto' || oy === 'scroll') && box.scrollHeight > box.clientHeight) break;
    box = box.parentElement;
  }
  if (!box || box === document.body) return;
  const r = el.getBoundingClientRect();
  const b = box.getBoundingClientRect();
  const room = 12;
  if (r.bottom > b.bottom - room) box.scrollTop += r.bottom - b.bottom + room;
  else if (r.top < b.top + room) box.scrollTop -= b.top + room - r.top;
}

function start() {
  const vv = window.visualViewport;
  const root = document.documentElement;
  if (!vv || !window.matchMedia('(pointer: coarse)').matches) return;
  root.dataset.touch = '';

  // the tallest visible height seen in the current orientation = height without the keyboard
  let full = vv.height;
  let frame = 0;
  // what was last brought into view, and at which visible height
  let shown: { el: Element | null; h: number } = { el: null, h: 0 };

  const update = () => {
    frame = 0;
    // a page zoomed in anyway (accessibility zoom) is left alone
    if (vv.scale > 1.01) return;
    full = Math.max(full, vv.height, window.innerHeight);
    const keyboard = isEditable(document.activeElement) && full - vv.height > KEYBOARD_MIN;
    root.style.setProperty('--app-h', `${Math.round(vv.height)}px`);
    root.style.setProperty('--app-top', `${Math.round(Math.max(0, vv.offsetTop))}px`);
    if (keyboard) root.dataset.kb = '';
    else delete root.dataset.kb;
    // the screen just shrank above the keyboard (or focus moved): the focused field may now sit
    // under a sticky footer or the keyboard — scroll its own container to it, after the layout
    const el = document.activeElement;
    if (keyboard && (el !== shown.el || Math.round(vv.height) !== shown.h)) {
      shown = { el, h: Math.round(vv.height) };
      requestAnimationFrame(() => revealInScroller(el));
    } else if (!keyboard) shown = { el: null, h: 0 };
    // iOS scrolls the (fixed, non-scrollable) document to reveal a field: undo it
    if (!keyboard && (window.scrollY || window.scrollX)) window.scrollTo(0, 0);
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(update);
  };

  vv.addEventListener('resize', schedule);
  vv.addEventListener('scroll', schedule);
  window.addEventListener('focusin', schedule);
  // focus moves between fields without the keyboard closing: wait for the new target
  window.addEventListener('focusout', () => setTimeout(schedule, 50));
  window.addEventListener('orientationchange', () => {
    full = 0;
    setTimeout(schedule, 300);
  });
  update();

  // Safari: pinch zoom
  const block = (e: Event) => e.preventDefault();
  document.addEventListener('gesturestart', block, { passive: false });
  document.addEventListener('gesturechange', block, { passive: false });
  document.addEventListener(
    'touchmove',
    (e) => {
      if (e.touches.length > 1) e.preventDefault();
    },
    { passive: false },
  );
}

start();
