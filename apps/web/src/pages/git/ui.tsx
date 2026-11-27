import { type ReactNode } from 'react';
import { Spinner } from '../../components/ui';


// ---------- frame ----------

/** lazygit-style box: the title sits in the top border; the focused one is accented. */
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

export function Act({ children, onClick, danger, disabled, submit }: { k?: string; children: ReactNode; onClick?: () => void; danger?: boolean; disabled?: boolean; submit?: boolean }) {
  return (
    <button
      type={submit ? 'submit' : 'button'}
      disabled={disabled}
      onClick={onClick}
      className={`border px-2 leading-6 disabled:opacity-40 ${danger ? 'border-danger/50 text-danger hover:bg-danger/10' : 'border-line hover:border-accent hover:text-accent'}`}
    >
      {children}
    </button>
  );
}

export function Acts({ children }: { children: ReactNode }) {
  return <div className="flex flex-wrap gap-1.5 py-1">{children}</div>;
}

