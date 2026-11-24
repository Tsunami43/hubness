import { useState } from 'react';
import { FileText } from 'lucide-react';
import type { GitStatus } from '@hubness/shared';
import { api } from '../../lib/api';
import { useApi } from '../../lib/query';
import { RepoContext, type Repo } from '../../lib/repo';
import { t } from '../../i18n';
import { FileIcon } from '../FileIcon';
import { Markdown } from '../Markdown';
import { Empty } from '../ui';
import { GitPage } from '../../pages/GitPage';

/** Side bar Git view: in a group, a repository switcher. */
export function GitTool({ repos, git, refresh }: { repos: Repo[]; git: Record<string, GitStatus | null>; refresh: () => void }) {
  const [current, setCurrent] = useState<string | null>(null);
  const repo = repos.find((r) => r.id === current) ?? repos[0];
  if (!repo) return <Empty title={t('groups.noRepos')} />;
  return (
    <RepoContext.Provider value={{ repo, git: git[repo.id] ?? null, refreshGit: refresh }}>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {repos.length > 1 ? (
          <div className="flex overflow-x-auto border-b border-line [scrollbar-width:none]">
            {repos.map((r) => (
              <button
                key={r.id}
                type="button"
                onClick={() => setCurrent(r.id)}
                className={`inline-flex shrink-0 items-center gap-1.5 border-b-2 px-3 py-1.5 text-[13px] ${repo.id === r.id ? 'border-accent text-fg' : 'border-transparent text-muted hover:text-fg'}`}
              >
                {r.name}
                {git[r.id]?.files.length ? <span className="text-xs text-warn">{git[r.id]!.files.length}</span> : null}
              </button>
            ))}
          </div>
        ) : null}
        <GitPage key={repo.id} />
      </div>
    </RepoContext.Provider>
  );
}

/** Empty editor: the README of the first repository (as on GitHub). */
export function Readme({ repo, multi }: { repo: Repo; multi: boolean }) {
  const { data } = useApi(['readme', repo.id], () =>
    api.tree(repo.id, '').then((list) => {
      const f = list.find((e) => !e.dir && /^readme(\.md|\.markdown|\.txt)?$/i.test(e.name));
      return f ? api.file(repo.id, f.path) : null;
    }),
  );
  if (!data) return <Empty icon={<FileText className="size-8" />} title={t('files.pick')} hint={t('files.pickHint')} />;
  return (
    <div className="min-h-0 flex-1">
      <div className="flex items-center gap-2 border-b border-line bg-surface px-3 py-2 text-[13px]">
        <FileIcon name={data.path} />
        <span className="text-muted">{multi ? `${repo.name}/` : ''}</span>
        {data.path}
      </div>
      <div className="mx-auto max-w-3xl px-5 py-4">
        <Markdown text={data.content} />
      </div>
    </div>
  );
}
