import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

/**
 * Themed tooltips for every `title` in the app: the native ones are drawn by the OS
 * (white bubbles, own font). On hover the title moves to data-tip (so the browser stays
 * quiet) and a small box in the theme shows it. Touch devices have no hover — nothing changes.
 */
export function TooltipHost() {
  const [tip, setTip] = useState<{ text: string; x: number; y: number; below: boolean } | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const [left, setLeft] = useState<number | null>(null);
  // keep the box inside the window (buttons at the edges)
  useLayoutEffect(() => {
    if (!tip || !box.current) return setLeft(null);
    const w = box.current.offsetWidth;
    setLeft(Math.max(8, Math.min(tip.x - w / 2, window.innerWidth - w - 8)));
  }, [tip]);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    let target: HTMLElement | null = null;
    const hide = () => {
      if (timer) clearTimeout(timer);
      timer = null;
      target = null;
      setTip(null);
    };
    const over = (e: PointerEvent) => {
      if (e.pointerType === 'touch') return;
      const el = (e.target as HTMLElement).closest?.<HTMLElement>('[title], [data-tip]');
      if (el === target) return;
      hide();
      if (!el) return;
      const t = el.getAttribute('title');
      if (t) {
        el.setAttribute('data-tip', t);
        el.removeAttribute('title');
        // the title was the only name of an icon button: keep it for screen readers
        if (!el.getAttribute('aria-label') && !el.textContent?.trim()) el.setAttribute('aria-label', t);
      }
      const text = el.getAttribute('data-tip');
      if (!text) return;
      target = el;
      timer = setTimeout(() => {
        if (target !== el || !el.isConnected) return;
        const r = el.getBoundingClientRect();
        const below = r.bottom + 40 < window.innerHeight;
        setTip({ text, x: r.left + r.width / 2, y: below ? r.bottom + 6 : r.top - 6, below });
      }, 450);
    };
    window.addEventListener('pointerover', over, true);
    window.addEventListener('pointerdown', hide, true);
    window.addEventListener('scroll', hide, true);
    window.addEventListener('keydown', hide, true);
    return () => {
      hide();
      window.removeEventListener('pointerover', over, true);
      window.removeEventListener('pointerdown', hide, true);
      window.removeEventListener('scroll', hide, true);
      window.removeEventListener('keydown', hide, true);
    };
  }, []);
  if (!tip) return null;
  return createPortal(
    <div
      ref={box}
      role="tooltip"
      className="pointer-events-none fixed z-[80] max-w-80 border border-line bg-surface px-2 py-1 text-[12px] leading-4 text-fg shadow-lg shadow-black/40"
      style={{
        left: left ?? tip.x,
        top: tip.y,
        visibility: left === null ? 'hidden' : 'visible',
        transform: tip.below ? undefined : 'translateY(-100%)',
      }}
    >
      {tip.text}
    </div>,
    document.body,
  );
}
