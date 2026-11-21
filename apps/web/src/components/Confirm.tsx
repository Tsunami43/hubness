import { useEffect, useSyncExternalStore } from 'react';
import { t } from '../i18n';
import { Button } from './ui';

/**
 * In-app replacement for window.confirm: `await confirmDialog({ message })` resolves to
 * true/false; `choiceDialog` offers several buttons and resolves to the chosen id (null —
 * cancelled). One <ConfirmHost/> is mounted at the root; requests queue up.
 */

export interface ConfirmOptions {
  message: string;
  /** Short title above the message; defaults to none. */
  title?: string;
  confirmLabel?: string;
  /** Destructive action: the confirm button is red. */
  danger?: boolean;
}

export interface Choice<T extends string = string> {
  id: T;
  label: string;
  variant?: 'primary' | 'secondary' | 'danger';
}

interface Pending {
  message: string;
  title?: string;
  choices: Choice[];
  resolve: (id: string | null) => void;
}

let queue: Pending[] = [];
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** Several actions plus Cancel; the last choice is the default (focused) one. */
export function choiceDialog<T extends string>(opts: { message: string; title?: string; choices: Choice<T>[] }): Promise<T | null> {
  return new Promise((resolve) => {
    queue = [...queue, { ...opts, resolve: resolve as (id: string | null) => void }];
    emit();
  });
}

export async function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
  const id = await choiceDialog({
    message: opts.message,
    title: opts.title,
    choices: [{ id: 'ok', label: opts.confirmLabel ?? t('common.ok'), variant: opts.danger ? 'danger' : 'primary' }],
  });
  return id === 'ok';
}

function settle(id: string | null) {
  const [head, ...rest] = queue;
  if (!head) return;
  queue = rest;
  emit();
  head.resolve(id);
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export function ConfirmHost() {
  const current = useSyncExternalStore(subscribe, () => queue[0] ?? null);
  useEffect(() => {
    if (!current) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') settle(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [current]);
  if (!current) return null;
  return (
    // a small centered dialog on every screen size: a yes/no question is not worth a full page
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50 p-4" onClick={() => settle(null)}>
      <div
        role="alertdialog"
        aria-modal="true"
        aria-label={current.title ?? current.message}
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-sm border border-line bg-surface shadow-2xl shadow-black/50"
      >
        <div className="px-4 pt-4 pb-3">
          {current.title ? <h2 className="mb-1 font-semibold text-accent">{current.title}</h2> : null}
          <p className="text-sm break-words whitespace-pre-line">{current.message}</p>
        </div>
        <div className="flex flex-wrap justify-end gap-2 border-t border-line px-4 py-3">
          <Button variant="ghost" onClick={() => settle(null)}>
            {t('common.cancel')}
          </Button>
          {current.choices.map((c, i) => (
            <Button key={c.id} autoFocus={i === current.choices.length - 1} variant={c.variant ?? 'secondary'} onClick={() => settle(c.id)}>
              {c.label}
            </Button>
          ))}
        </div>
      </div>
    </div>
  );
}
