import { useEffect, useRef, useState, type ReactNode } from 'react';
import { CaseSensitive, ChevronRight, ChevronsDownUp, Ellipsis, Regex, WholeWord, X } from 'lucide-react';
import type { SearchFile } from '@hubness/shared';
import type { Repo } from '../lib/repo';
import { api } from '../lib/api';
import { useWorkbench } from '../lib/workbench';
import { t } from '../i18n';
import { FileIcon } from './FileIcon';
import { ErrorLine, Spinner } from './ui';

/**
 * Search view (like VS Code's): text across all project files, with match case / whole
 * word / regex toggles and include/exclude globs. Clicking a match opens the file at the
 * line (preview tab; double click keeps it open). In a group, every repository is searched.
 */

interface Hit extends SearchFile {
  repo: string;
  repoName: string;
}

export function SearchView({ repos, focusNonce }: { repos: Repo[]; focusNonce?: number }) {
  const wb = useWorkbench();
  const input = useRef<HTMLInputElement>(null);
  const [q, setQ] = useState('');
  const [caseSensitive, setCase] = useState(false);
  const [word, setWord] = useState(false);
  const [regex, setRegex] = useState(false);
  const [details, setDetails] = useState(false);
  const [include, setInclude] = useState('');
  const [exclude, setExclude] = useState('');
  const [hits, setHits] = useState<Hit[] | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (focusNonce) {
      input.current?.focus();
      input.current?.select();
    }
  }, [focusNonce]);

  // search as you type (debounced); a newer query cancels the older one
  useEffect(() => {
    if (!q.trim()) {
      setHits(null);
      setError(null);
      setBusy(false);
      return;
    }
    let alive = true;
    setBusy(true);
    const timer = setTimeout(() => {
      Promise.all(
        repos.map((r) =>
          api.search(r.id, { q, caseSensitive, word, regex, include, exclude }).then((res) => ({ res, r })),
        ),
      ).then(
        (all) => {
          if (!alive) return;
          setHits(all.flatMap(({ res, r }) => res.files.map((f) => ({ ...f, repo: r.id, repoName: r.name }))));
          setTruncated(all.some(({ res }) => res.truncated));
          setError(null);
          setCollapsed(new Set());
          setBusy(false);
        },
        (e) => {
          if (!alive) return;
          setError((e as Error).message);
          setHits(null);
          setBusy(false);
        },
      );
    }, 300);
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [q, caseSensitive, word, regex, include, exclude, repos]);

  const total = hits?.reduce((n, f) => n + f.matches.length, 0) ?? 0;
  const multi = repos.length > 1;
  const open = (h: Hit, line: number, pinned: boolean) => wb?.openFile(h.repo, h.path, { preview: !pinned, line });

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="shrink-0 space-y-1.5 border-b border-line p-2">
        <div className="flex items-start gap-1">
          <div className="flex min-w-0 flex-1 items-center border border-line bg-bg focus-within:border-accent">
            <input
              ref={input}
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder={t('search.placeholder')}
              className="min-w-0 flex-1 bg-transparent px-2 py-1.5 text-[13px] outline-none"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              onKeyDown={(e) => e.key === 'Escape' && setQ('')}
            />
            <Toggle on={caseSensitive} onClick={() => setCase(!caseSensitive)} label={t('search.case')}>
              <CaseSensitive className="size-4" />
            </Toggle>
            <Toggle on={word} onClick={() => setWord(!word)} label={t('search.word')}>
              <WholeWord className="size-4" />
            </Toggle>
            <Toggle on={regex} onClick={() => setRegex(!regex)} label={t('search.regex')}>
              <Regex className="size-4" />
            </Toggle>
          </div>
          <Toggle on={details} onClick={() => setDetails(!details)} label={t('search.details')} boxed>
            <Ellipsis className="size-4" />
          </Toggle>
        </div>
        {details ? (
          <>
            <GlobInput value={include} onChange={setInclude} placeholder={t('search.include')} />
            <GlobInput value={exclude} onChange={setExclude} placeholder={t('search.exclude')} />
          </>
        ) : null}
        {q.trim() ? (
          <div className="flex items-center gap-1 text-xs text-muted">
            <span className="min-w-0 flex-1 truncate">
              {busy ? <Spinner className="size-3" /> : hits ? (total ? t('search.summary', { n: total, files: hits.length }) : t('search.none')) : null}
            </span>
            {hits?.length ? (
              <button type="button" title={t('search.collapse')} aria-label={t('search.collapse')} onClick={() => setCollapsed(new Set(hits.map((h) => `${h.repo}:${h.path}`)))} className="p-0.5 hover:text-fg">
                <ChevronsDownUp className="size-3.5" />
              </button>
            ) : null}
            <button type="button" title={t('search.clear')} aria-label={t('search.clear')} onClick={() => setQ('')} className="p-0.5 hover:text-fg">
              <X className="size-3.5" />
            </button>
          </div>
        ) : null}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        <div className="px-2 empty:hidden">
          <ErrorLine error={error} />
        </div>
        {!q.trim() ? <p className="px-3 py-2 text-xs text-muted">{t('search.hint')}</p> : null}
        {truncated ? <p className="px-3 py-1 text-xs text-warn">{t('search.truncated', { n: total })}</p> : null}
        {hits?.map((h) => {
          const key = `${h.repo}:${h.path}`;
          const shut = collapsed.has(key);
          const i = h.path.lastIndexOf('/');
          return (
            <div key={key}>
              <button
                type="button"
                onClick={() => setCollapsed((s) => (s.has(key) ? new Set([...s].filter((x) => x !== key)) : new Set(s).add(key)))}
                className="flex w-full items-center gap-1.5 py-[3px] pr-2 pl-1.5 text-left text-[13px] hover:bg-sunken"
                title={h.path}
              >
                <ChevronRight className={`size-3.5 shrink-0 text-muted transition-transform ${shut ? '' : 'rotate-90'}`} />
                <FileIcon name={h.path.slice(i + 1)} />
                <span className="shrink-0">{h.path.slice(i + 1)}</span>
                <span className="min-w-0 flex-1 truncate text-xs text-muted">
                  {multi ? `${h.repoName}/` : ''}
                  {i > 0 ? h.path.slice(0, i) : ''}
                </span>
                <span className="shrink-0 bg-sunken px-1.5 text-[11px] text-muted">{h.matches.length}</span>
              </button>
              {shut
                ? null
                : h.matches.map((m, j) => (
                    <button
                      key={j}
                      type="button"
                      onClick={() => open(h, m.line, false)}
                      onDoubleClick={() => open(h, m.line, true)}
                      className="flex w-full items-baseline gap-2 py-[2px] pr-2 pl-9 text-left font-mono text-[12px] hover:bg-sunken"
                    >
                      <span className="min-w-0 flex-1 truncate whitespace-pre">
                        <Line text={m.text} ranges={m.ranges} />
                      </span>
                      <span className="shrink-0 text-[11px] text-muted/70">{m.line}</span>
                    </button>
                  ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Match line: leading whitespace trimmed, a long prefix cut so the first match is visible. */
function Line({ text, ranges }: { text: string; ranges: [number, number][] }) {
  const lead = text.length - text.trimStart().length;
  const first = ranges[0]?.[0] ?? 0;
  const from = Math.max(lead, first - 24);
  const parts: ReactNode[] = [];
  let at = from;
  if (from > lead) parts.push('…');
  for (const [a, b] of ranges) {
    if (b <= at) continue;
    const s = Math.max(a, at);
    if (s > at) parts.push(text.slice(at, s));
    parts.push(
      <mark key={a} className="bg-warn/30 text-fg">
        {text.slice(s, b)}
      </mark>,
    );
    at = b;
  }
  parts.push(text.slice(at));
  return <>{parts}</>;
}

function Toggle({ on, onClick, label, children, boxed }: { on: boolean; onClick: () => void; label: string; children: ReactNode; boxed?: boolean }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      aria-pressed={on}
      onClick={onClick}
      className={`inline-flex size-7 shrink-0 items-center justify-center ${boxed ? 'border border-transparent' : ''} ${on ? 'bg-accent/20 text-accent' : 'text-muted hover:text-fg'}`}
    >
      {children}
    </button>
  );
}

function GlobInput({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder: string }) {
  return (
    <input
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      className="w-full border border-line bg-bg px-2 py-1 font-mono text-[12px] outline-none focus:border-accent"
      autoCapitalize="off"
      autoCorrect="off"
      spellCheck={false}
    />
  );
}
