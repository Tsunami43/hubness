import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown } from 'lucide-react';

/**
 * Themed replacement for <select>: the native dropdown is drawn by the OS and ignores the
 * theme. Keyboard: Enter/Space/↓ opens, ↑/↓ move, Enter picks, Esc closes, letters jump.
 */
export interface SelectOption<T extends string> {
  value: T;
  label: string;
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  label,
  className = '',
  size = 'md',
}: {
  value: T;
  options: SelectOption<T>[];
  onChange: (v: T) => void;
  /** Accessible name (no visible label). */
  label?: string;
  className?: string;
  size?: 'sm' | 'md';
}) {
  const [open, setOpen] = useState(false);
  const [hi, setHi] = useState(0);
  const btn = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number; width: number; up: boolean } | null>(null);
  const current = options.find((o) => o.value === value) ?? options[0];

  const place = () => {
    const r = btn.current?.getBoundingClientRect();
    if (!r) return;
    const below = window.innerHeight - r.bottom;
    const up = below < 220 && r.top > below;
    setPos({ left: r.left, top: up ? r.top : r.bottom, width: r.width, up });
  };
  useLayoutEffect(() => {
    if (!open) return;
    place();
    setHi(Math.max(0, options.findIndex((o) => o.value === value)));
    const close = (e: Event) => {
      if (btn.current?.contains(e.target as Node) || list.current?.contains(e.target as Node)) return;
      setOpen(false);
    };
    const reflow = () => place();
    window.addEventListener('pointerdown', close, true);
    window.addEventListener('resize', reflow);
    window.addEventListener('scroll', reflow, true);
    return () => {
      window.removeEventListener('pointerdown', close, true);
      window.removeEventListener('resize', reflow);
      window.removeEventListener('scroll', reflow, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  useEffect(() => {
    if (open) list.current?.children[hi]?.scrollIntoView({ block: 'nearest' });
  }, [hi, open]);

  const pick = (v: T) => {
    setOpen(false);
    btn.current?.focus();
    if (v !== value) onChange(v);
  };
  const onKey = (e: KeyboardEvent) => {
    if (!open) {
      if (['Enter', ' ', 'ArrowDown', 'ArrowUp'].includes(e.key)) {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (e.key === 'Escape' || e.key === 'Tab') return setOpen(false);
    if (e.key === 'ArrowDown') return e.preventDefault(), setHi((i) => Math.min(options.length - 1, i + 1));
    if (e.key === 'ArrowUp') return e.preventDefault(), setHi((i) => Math.max(0, i - 1));
    if (e.key === 'Home') return e.preventDefault(), setHi(0);
    if (e.key === 'End') return e.preventDefault(), setHi(options.length - 1);
    if (e.key === 'Enter' || e.key === ' ') return e.preventDefault(), options[hi] && pick(options[hi].value);
    if (e.key.length === 1) {
      const i = options.findIndex((o, n) => n > hi && o.label.toLowerCase().startsWith(e.key.toLowerCase()));
      const j = i >= 0 ? i : options.findIndex((o) => o.label.toLowerCase().startsWith(e.key.toLowerCase()));
      if (j >= 0) setHi(j);
    }
  };

  const h = size === 'sm' ? 'py-0.5 text-[13px]' : 'py-1.5 text-sm';
  return (
    <>
      <button
        ref={btn}
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={label}
        onClick={() => setOpen(!open)}
        onKeyDown={onKey}
        className={`inline-flex min-w-0 items-center gap-2 border bg-surface px-2.5 text-left ${h} ${open ? 'border-accent' : 'border-line hover:border-muted'} ${className}`}
      >
        <span className="min-w-0 flex-1 truncate">{current?.label}</span>
        <ChevronDown className={`size-3.5 shrink-0 text-muted transition ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && pos
        ? createPortal(
            <ul
              ref={list}
              role="listbox"
              aria-label={label}
              className="fixed z-[70] max-h-64 overflow-y-auto border border-accent bg-surface py-1 shadow-xl shadow-black/40"
              style={{
                left: Math.min(pos.left, window.innerWidth - Math.max(pos.width, 160) - 8),
                minWidth: Math.max(pos.width, 160),
                ...(pos.up ? { bottom: window.innerHeight - pos.top + 2 } : { top: pos.top + 2 }),
              }}
            >
              {options.map((o, i) => (
                <li
                  key={o.value}
                  role="option"
                  aria-selected={o.value === value}
                  onPointerEnter={() => setHi(i)}
                  onClick={() => pick(o.value)}
                  className={`flex cursor-pointer items-center gap-2 px-2.5 py-1.5 text-sm ${i === hi ? 'bg-accent/20 text-fg' : 'text-fg/90'}`}
                >
                  <Check className={`size-3.5 shrink-0 ${o.value === value ? 'text-accent' : 'invisible'}`} />
                  <span className="truncate">{o.label}</span>
                </li>
              ))}
            </ul>,
            document.body,
          )
        : null}
    </>
  );
}
