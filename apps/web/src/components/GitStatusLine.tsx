import { ArrowDown, ArrowUp, GitBranch } from 'lucide-react';
import type { GitStatus } from '@hubness/shared';
import type { Repo } from '../lib/repo';
import { t } from '../i18n';

/** Changes of a working tree by kind, for the short summary. */
export function countChanges(g: GitStatus) {
  let staged = 0;
  let modified = 0;
  let untracked = 0;
  let conflicts = 0;
  for (const f of g.files) {
    if (f.conflicted) conflicts++;
    else if (f.untracked) untracked++;
    else {
      if (f.index !== ' ' && f.index !== '?') staged++;
      if (f.worktree !== ' ' && f.worktree !== '?') modified++;
    }
  }
  return { staged, modified, untracked, conflicts };
}

/**
 * The bottom line of the side bar: the branch the working tree is on and a short summary of
 * its changes (one line per repository in a group). A click opens Source control.
 */
export function GitStatusLine({ repos, git, onOpen }: { repos: Repo[]; git: Record<string, GitStatus | null>; onOpen: () => void }) {
  const rows = repos.map((r) => ({ repo: r, g: git[r.id] })).filter((x): x is { repo: Repo; g: GitStatus } => Boolean(x.g?.isRepo));
  if (!rows.length) return null;
  const multi = repos.length > 1;
  return (
    <div className="shrink-0 border-t border-line">
      {rows.map(({ repo, g }) => {
        const c = countChanges(g);
        const branch = g.branch ?? t('gitline.detached');
        const parts = [
          c.conflicts ? { n: c.conflicts, mark: '!', cls: 'text-danger', label: t('gitline.conflicts', { n: c.conflicts }) } : null,
          c.staged ? { n: c.staged, mark: '+', cls: 'text-ok', label: t('gitline.staged', { n: c.staged }) } : null,
          c.modified ? { n: c.modified, mark: '~', cls: 'text-warn', label: t('gitline.modified', { n: c.modified }) } : null,
          c.untracked ? { n: c.untracked, mark: '?', cls: 'text-muted', label: t('gitline.untracked', { n: c.untracked }) } : null,
        ].filter((p) => p !== null);
        const title = [
          `${multi ? `${repo.name}: ` : ''}${branch}${g.upstream ? ` → ${g.upstream}` : ''}`,
          g.ahead ? t('gitline.ahead', { n: g.ahead }) : '',
          g.behind ? t('gitline.behind', { n: g.behind }) : '',
          g.state ? t(`git.state.${g.state}`) : '',
          ...parts.map((p) => p.label),
          parts.length ? '' : t('git.clean'),
        ]
          .filter(Boolean)
          .join('\n');
        return (
          <button
            key={repo.id}
            type="button"
            onClick={onOpen}
            title={title}
            className="flex h-7 w-full min-w-0 items-center gap-2 px-3 text-left font-mono text-xs text-muted hover:bg-sunken hover:text-fg"
          >
            <GitBranch className="size-3.5 shrink-0" style={{ color: 'var(--color-accent)' }} />
            {multi ? <span className="min-w-0 shrink truncate">{repo.name}:</span> : null}
            <span className={`min-w-0 shrink truncate ${g.branch ? 'text-fg' : 'italic'}`}>{branch}</span>
            {g.ahead ? (
              <span className="inline-flex shrink-0 items-center">
                <ArrowUp className="size-3" />
                {g.ahead}
              </span>
            ) : null}
            {g.behind ? (
              <span className="inline-flex shrink-0 items-center">
                <ArrowDown className="size-3" />
                {g.behind}
              </span>
            ) : null}
            {g.state ? <span className="shrink-0 text-warn">{g.state}</span> : null}
            <span className="ml-auto flex shrink-0 items-center gap-2">
              {parts.map((p) => (
                <span key={p.mark} className={p.cls}>
                  {p.mark}
                  {p.n}
                </span>
              ))}
            </span>
          </button>
        );
      })}
    </div>
  );
}
