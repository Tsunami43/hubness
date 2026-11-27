import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useSearchParams } from 'react-router';
import { Info, MoreHorizontal, Search, X } from 'lucide-react';
import type { AgentKind } from '@hubness/shared';
import { t } from '../../i18n';
import { AgentIcon } from '../AgentIcon';
import { ProjectSelect } from '../HarnessExtensions';
import { ErrorLine, Sheet, Spinner } from '../ui';

/**
 * Building blocks of the Harness → Extensions pages (MCP servers, skills, rules, subagents,
 * commands, plugins): one card per page with a help popover, a project picker and a search
 * box, a matrix "item × agent", row menus and a read-only file viewer.
 */

/** The project picked on an extensions page, kept in `?project=` so links and reloads keep it. */
export function useProjectParam(): [string, (id: string) => void] {
  const [params, setParams] = useSearchParams();
  const project = params.get('project') ?? '';
  const set = (id: string) =>
    setParams(
      (cur) => {
        const next = new URLSearchParams(cur);
        if (id) next.set('project', id);
        else next.delete('project');
        return next;
      },
      { replace: true },
    );
  return [project, set];
}

/** Does any of the fields contain the query (case-insensitive)? */
export const matches = (q: string, ...fields: (string | null | undefined)[]) => {
  const s = q.trim().toLowerCase();
  return !s || fields.some((f) => f?.toLowerCase().includes(s));
};

/** An ⓘ button with a popover that explains the concept of the page in plain words. */
export function InfoButton({ title, paragraphs }: { title: string; paragraphs: string[] }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const off = (e: Event) => {
      if (e instanceof KeyboardEvent ? e.key === 'Escape' : !ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener('mousedown', off);
    window.addEventListener('keydown', off);
    return () => {
      window.removeEventListener('mousedown', off);
      window.removeEventListener('keydown', off);
    };
  }, [open]);
  return (
    <span ref={ref} className="relative inline-flex">
      <button
        type="button"
        aria-label={title}
        aria-expanded={open}
        title={title}
        onClick={() => setOpen((v) => !v)}
        className={`inline-flex size-7 items-center justify-center rounded-full ${open ? 'bg-sunken text-fg' : 'text-muted hover:bg-sunken hover:text-fg'}`}
      >
        <Info className="size-4" />
      </button>
      {open ? (
        <div role="dialog" aria-label={title} className="ui-solid absolute top-8 left-0 z-40 w-[min(26rem,calc(100vw-2rem))] border border-line bg-surface p-4 text-sm shadow-xl shadow-black/30 max-sm:-left-24">
          <div className="mb-2 flex items-start gap-2">
            <h3 className="flex-1 font-semibold">{title}</h3>
            <button type="button" aria-label={t('common.close')} onClick={() => setOpen(false)} className="text-muted hover:text-fg">
              <X className="size-4" />
            </button>
          </div>
          <div className="space-y-2 text-[13px] leading-relaxed text-muted">
            {paragraphs.map((p) => (
              <p key={p}>{p}</p>
            ))}
          </div>
        </div>
      ) : null}
    </span>
  );
}

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onClick: () => void;
}

/** A ⋯ button with a small menu of row actions. */
export function RowMenu({ label, items }: { label: string; items: MenuItem[] }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const off = (e: Event) => {
      if (e instanceof KeyboardEvent ? e.key === 'Escape' : !ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener('mousedown', off);
    window.addEventListener('keydown', off);
    return () => {
      window.removeEventListener('mousedown', off);
      window.removeEventListener('keydown', off);
    };
  }, [open]);
  return (
    <span ref={ref} className="relative inline-flex">
      <button
        type="button"
        aria-label={label}
        title={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex size-7 items-center justify-center rounded-md text-muted hover:bg-sunken hover:text-fg"
      >
        <MoreHorizontal className="size-4" />
      </button>
      {open ? (
        <div role="menu" className="ui-solid absolute top-8 right-0 z-30 min-w-52 border border-line bg-surface py-1 text-left text-[13px] shadow-lg">
          {items.map((it) => (
            <button
              key={it.label}
              type="button"
              role="menuitem"
              disabled={it.disabled}
              onClick={() => {
                setOpen(false);
                it.onClick();
              }}
              className={`flex w-full items-center gap-2 px-3 py-1.5 hover:bg-sunken disabled:opacity-40 disabled:hover:bg-transparent ${it.danger ? 'text-danger' : ''}`}
            >
              {it.icon ? <span className="flex size-4 items-center justify-center">{it.icon}</span> : null}
              <span className="flex-1">{it.label}</span>
            </button>
          ))}
        </div>
      ) : null}
    </span>
  );
}

/** An on/off switch of one matrix cell. */
export function Toggle({ on, disabled, title, label, onClick }: { on: boolean; disabled?: boolean; title: string; label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      title={title}
      disabled={disabled}
      onClick={onClick}
      className={`flex h-5 w-9 shrink-0 items-center rounded-full border transition disabled:opacity-40 ${on ? 'border-accent bg-accent' : 'border-line bg-sunken'}`}
    >
      <span className={`size-3.5 rounded-full bg-surface shadow transition ${on ? 'translate-x-4.5' : 'translate-x-0.5'}`} />
    </button>
  );
}

/** The agents as matrix column headers. */
export function AgentHeads({ agents }: { agents: { kind: AgentKind; label: string }[] }) {
  return (
    <>
      {agents.map((a) => (
        <th key={a.kind} className="px-2 py-2 text-center font-normal">
          <span className="inline-flex flex-col items-center gap-1">
            <AgentIcon kind={a.kind} className="size-4" />
            <span className="whitespace-nowrap">{a.label}</span>
          </span>
        </th>
      ))}
    </>
  );
}

/**
 * One extensions page: a card with the title, the help popover, the project picker and the
 * page's actions, a search box and the body.
 */
export function ExtPage({
  title,
  intro,
  help,
  actions,
  project,
  onProject,
  query,
  onQuery,
  searchPlaceholder,
  error,
  loading,
  children,
  footer,
}: {
  title: string;
  intro: string;
  help: { title: string; paragraphs: string[] };
  actions?: ReactNode;
  project: string;
  onProject: (id: string) => void;
  query: string;
  onQuery: (q: string) => void;
  searchPlaceholder: string;
  error?: string | null;
  loading?: boolean;
  children: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <div className="space-y-3">
      <section className="rounded-2xl border border-line bg-surface">
        <header className="flex flex-col gap-2 px-4 pt-4 sm:flex-row sm:items-start sm:px-5">
          <div className="min-w-0 sm:flex-1">
            <div className="flex items-center gap-1">
              <h2 className="text-[17px] font-semibold">{title}</h2>
              <InfoButton title={help.title} paragraphs={help.paragraphs} />
            </div>
            <p className="text-xs text-muted">{intro}</p>
          </div>
          {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
        </header>
        <div className="flex flex-wrap items-center gap-2 px-4 py-3 sm:px-5">
          <label className="relative min-w-40 flex-1">
            <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted" />
            <input
              type="search"
              value={query}
              onChange={(e) => onQuery(e.target.value)}
              onKeyDown={(e) => e.key === 'Escape' && onQuery('')}
              placeholder={searchPlaceholder}
              aria-label={searchPlaceholder}
              className="h-8 w-full border border-line bg-bg pr-2 pl-8 text-sm outline-none placeholder:text-muted/70 focus:border-accent"
            />
          </label>
          <ProjectSelect value={project} onChange={onProject} />
        </div>
        <div className="border-t border-line/60">
          {error ? (
            <div className="p-4">
              <ErrorLine error={error} />
            </div>
          ) : loading ? (
            <div className="flex justify-center py-8">
              <Spinner />
            </div>
          ) : (
            children
          )}
        </div>
      </section>
      {footer}
    </div>
  );
}

/** "Nothing matches" / "nothing yet" row of a matrix. */
export function EmptyRow({ cols, text }: { cols: number; text: string }) {
  return (
    <tr>
      <td colSpan={cols} className="px-4 py-5 text-sm text-muted sm:px-5">
        {text}
      </td>
    </tr>
  );
}

/** A read-only text file in a sheet (SKILL.md, a subagent, a command). */
export function FileViewer({
  open,
  title,
  path,
  content,
  loading,
  error,
  note,
  aside,
  onClose,
}: {
  open: boolean;
  title: string;
  path?: string;
  content?: string;
  loading?: boolean;
  error?: string | null;
  note?: string;
  aside?: ReactNode;
  onClose: () => void;
}) {
  return (
    <Sheet open={open} onClose={onClose} title={title} wide>
      <div className="space-y-3">
        {path ? <div className="font-mono text-[11px] break-all text-muted">{path}</div> : null}
        {note ? <p className="text-xs text-muted">{note}</p> : null}
        {error ? <ErrorLine error={error} /> : null}
        {loading ? (
          <div className="flex justify-center py-6">
            <Spinner />
          </div>
        ) : content !== undefined ? (
          <pre className="max-h-[60dvh] overflow-auto border border-line bg-bg p-3 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap">{content}</pre>
        ) : null}
        {aside}
      </div>
    </Sheet>
  );
}

/** The error text of a failed request. */
export const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** An HTTP status in an API error, when the client puts it there (409 → replace?). */
export const isConflict = (e: unknown) => e instanceof Error && 'status' in e && (e as { status?: number }).status === 409;
