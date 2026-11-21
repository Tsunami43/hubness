import { useEffect, useRef, type ReactNode } from 'react';
import { Spinner } from '../../components/ui';
import { type PanelId } from './types';


// ---------- frame ----------

/** lazygit-style box: the title sits in the top border; the focused one is accented. */
export function Panel({
  n,
  title,
  focus,
  wide,
  setFocus,
  count,
  extra,
  grow,
  main,
  summary,
  children,
}: {
  summary?: ReactNode;
  n: number;
  title: string;
  focus: PanelId;
  wide: boolean;
  setFocus: (p: PanelId) => void;
  count?: number;
  extra?: ReactNode;
  grow?: boolean;
  main?: boolean;
  children: ReactNode;
}) {
  const active = main || focus === n;
  // narrow: accordion — only the focused panel shows its body
  const open = wide || active;
  const height = main ? '' : wide ? (active ? (grow ? 'max-h-[60vh]' : 'max-h-[45vh]') : 'max-h-[24vh]') : '';
  return (
    <section className={`relative border ${active ? 'border-accent' : 'border-line'} bg-surface`}>
      <div className="absolute -top-2.5 right-2 left-2 flex items-center gap-2 leading-5">
        <button type="button" onClick={() => !main && setFocus(n as PanelId)} className="flex min-w-0 items-center gap-1 bg-surface px-1">
          {main ? null : <span className={active ? 'text-accent' : 'text-muted'}>[{n}]</span>}
          <span className={`truncate ${active ? 'font-semibold text-fg' : 'text-muted'}`}>{title}</span>
          {count ? <span className="text-muted">({count})</span> : null}
        </button>
        {extra && open ? <span className="ml-auto bg-surface px-1">{extra}</span> : null}
      </div>
      {open ? (
        <div className={`overflow-y-auto pt-3 pb-1 ${height}`}>{children}</div>
      ) : (
        <button type="button" onClick={() => setFocus(n as PanelId)} className="block w-full truncate px-3 pt-3 pb-1 text-left text-[12px] text-muted">
          {summary ?? '\u00a0'}
        </button>
      )}
    </section>
  );
}

export function Row({ selected, active, onClick, children }: { selected: boolean; active: boolean; onClick: () => void; children: ReactNode }) {
  const ref = useRef<HTMLLIElement>(null);
  useEffect(() => {
    if (selected && active) ref.current?.scrollIntoView({ block: 'nearest' });
  }, [selected, active]);
  return (
    <li ref={ref}>
      <div
        role="button"
        tabIndex={-1}
        onClick={(e) => {
          // clicks inside an unfolded detail must not toggle the row
          if ((e.target as HTMLElement).closest('[data-detail]')) return;
          onClick();
        }}
        className={`cursor-pointer px-3 leading-[22px] outline-none ${selected ? (active ? 'bg-accent/20' : 'bg-sunken') : 'hover:bg-sunken/70'}`}
      >
        {children}
      </div>
    </li>
  );
}

export function Detail({ children }: { children: ReactNode }) {
  return (
    <div data-detail className="-mx-3 cursor-default border-y border-line/60 bg-bg/60 px-3 py-2">
      {children}
    </div>
  );
}

export function Busy() {
  return (
    <div className="flex justify-center py-3">
      <Spinner />
    </div>
  );
}

export function Act({ k, children, onClick, danger, disabled, submit }: { k?: string; children: ReactNode; onClick?: () => void; danger?: boolean; disabled?: boolean; submit?: boolean }) {
  return (
    <button
      type={submit ? 'submit' : 'button'}
      disabled={disabled}
      onClick={onClick}
      className={`border px-2 leading-6 disabled:opacity-40 ${danger ? 'border-danger/50 text-danger hover:bg-danger/10' : 'border-line hover:border-accent hover:text-accent'}`}
    >
      {k ? <span className="mr-1 hidden text-accent sm:inline">{k}</span> : null}
      {children}
    </button>
  );
}

export function Acts({ children }: { children: ReactNode }) {
  return <div className="flex flex-wrap gap-1.5 py-1">{children}</div>;
}

export const HINTS: Record<PanelId, [string, string][]> = {
  1: [['f', 'fetch'], ['p', 'pull'], ['P', 'push']],
  2: [['space', 'stage'], ['a', 'all'], ['d', 'discard'], ['c', 'commit'], ['s', 'stash']],
  3: [['space', 'checkout'], ['M', 'merge'], ['r', 'rebase'], ['d', 'delete']],
  4: [['enter', 'details']],
  5: [['space', 'apply'], ['g', 'pop'], ['d', 'drop']],
};

export function KeyHints({ focus }: { focus: PanelId }) {
  return (
    <div className="mt-3 hidden flex-wrap gap-x-3 text-[11px] text-muted sm:flex">
      {[...HINTS[focus], ['1-5', 'panels'], ['j/k', 'move']].map(([k, v]) => (
        <span key={k + v}>
          <span className="text-accent">{k}</span> {v}
        </span>
      ))}
    </div>
  );
}
