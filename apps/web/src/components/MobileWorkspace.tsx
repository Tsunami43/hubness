import { useEffect, useState, type ReactNode } from 'react';
import { useSearchParams } from 'react-router';
import { ArrowDown, ArrowUp, Bot, ChevronLeft, ChevronRight, File, Folder, GitBranch, LayoutPanelLeft, Loader2, MessagesSquare, RefreshCw, Smartphone } from 'lucide-react';
import type { FsEntry, GitAuthProblem, GitFile, GitStatus } from '@hubness/shared';
import { api } from '../lib/api';
import { pushDraft } from '../lib/ask';
import type { SessionScope } from '../lib/hooks';
import { t } from '../i18n';
import { useAccess } from './AuthGate';
import { DiffView } from './DiffView';
import { FileViewer } from './FileViewer';
import { GitAuthNotice, GitLoginSheet } from './GitLogin';
import type { Tool } from './RepoWorkspace';
import { SessionsTool } from './SessionsTool';
import { Button, ErrorLine, Spinner, TopBar } from './ui';

type Repo = { id: string; name: string; path: string };
type Tab = 'chat' | 'changes' | 'files' | 'more';

/**
 * A project or group on a phone: a remote control for the agents rather than a small IDE.
 * Chat first; Changes shows what the agents changed (repo → files → diff) with "ask the agent
 * to commit & push", pull and push; Files is a read-first browser; More holds the other tools
 * and the full workspace. The tab lives in `?m=`, an open session in `?s=` (SessionsTool).
 */
export function MobileWorkspace({
  title,
  badge,
  subtitle,
  scope,
  root,
  repos,
  git,
  refreshGit,
  tools,
  onFull,
}: {
  title: string;
  badge?: string;
  subtitle?: ReactNode;
  scope: SessionScope;
  root: string;
  repos: Repo[];
  git: Record<string, GitStatus | null>;
  refreshGit: () => void;
  /** Tools besides sessions (Run, Shell, Settings…), listed under More. */
  tools: Tool[];
  /** Switch to the full (desktop-like) workspace. */
  onFull: () => void;
}) {
  const [params, setParams] = useSearchParams();
  const access = useAccess();
  const tab = (params.get('m') as Tab | null) ?? 'chat';
  const sessionOpen = tab === 'chat' && params.has('s');
  const scopeKey = 'projectId' in scope ? scope.projectId : `group:${scope.groupId}`;

  const go = (m: Tab, extra?: Record<string, string | null>) => {
    const next = new URLSearchParams(params);
    if (m === 'chat') next.delete('m');
    else next.set('m', m);
    for (const [k, v] of Object.entries(extra ?? {})) {
      if (v === null) next.delete(k);
      else next.set(k, v);
    }
    setParams(next, { replace: true });
  };
  // "commit & push" goes to the agent: a new session with the request typed in
  const askAgent = (text: string) => {
    pushDraft(`${scopeKey}:new`, text);
    go('chat', { s: 'new' });
  };

  const tabs: { id: Tab; label: string; icon: typeof MessagesSquare; badge?: number }[] = [
    { id: 'chat', label: t('mobile.chat'), icon: MessagesSquare },
    ...(access.can('git') ? [{ id: 'changes' as const, label: t('mobile.changes'), icon: GitBranch, badge: changedFiles(repos, git) }] : []),
    ...(access.can('files') ? [{ id: 'files' as const, label: t('mobile.files'), icon: Folder }] : []),
    { id: 'more', label: t('mobile.more'), icon: LayoutPanelLeft },
  ];

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* an open session has its own header with a back button */}
      {sessionOpen ? null : (
        <>
          <TopBar back="/projects" title={badge ? <span className="flex items-baseline gap-2">{title}<span className="border border-line px-1 text-[10px] font-normal text-muted">{badge}</span></span> : title} subtitle={subtitle} />
          <nav className="ui-tabs grid shrink-0 border-b border-line bg-surface" style={{ gridTemplateColumns: `repeat(${tabs.length}, minmax(0, 1fr))` }}>
            {tabs.map((x) => (
              <button
                key={x.id}
                type="button"
                onClick={() => go(x.id, { s: null })}
                aria-current={tab === x.id ? 'page' : undefined}
                className={`flex min-w-0 flex-col items-center justify-center gap-0.5 border-b-2 py-1.5 text-xs ${tab === x.id ? 'border-accent text-accent' : 'border-transparent text-muted'}`}
              >
                {/* icon over the label (four tabs fit a phone), the count as a badge on the icon */}
                <span className="relative">
                  <x.icon className="size-5" />
                  {x.badge ? <span className="absolute -top-1.5 left-3.5 min-w-4 rounded-full bg-accent px-1 text-center text-[10px] leading-4 text-accent-fg">{x.badge}</span> : null}
                </span>
                <span className="max-w-full truncate">{x.label}</span>
              </button>
            ))}
          </nav>
        </>
      )}
      <div className="flex min-h-0 flex-1 flex-col">
        {tab === 'chat' ? <SessionsTool scope={scope} root={root} title={title} /> : null}
        {tab === 'changes' ? <ChangesTab repos={repos} git={git} refresh={refreshGit} onAsk={askAgent} /> : null}
        {tab === 'files' ? <FilesTab repos={repos} git={git} /> : null}
        {tab === 'more' ? <MoreTab tools={tools} onFull={onFull} /> : null}
      </div>
    </div>
  );
}

const changedFiles = (repos: Repo[], git: Record<string, GitStatus | null>) => repos.reduce((n, r) => n + (git[r.id]?.files.length ?? 0), 0);

// ---------- Changes ----------

function ChangesTab({ repos, git, refresh, onAsk }: { repos: Repo[]; git: Record<string, GitStatus | null>; refresh: () => void; onAsk: (text: string) => void }) {
  const [open, setOpen] = useState<{ repo: Repo; file?: GitFile } | null>(repos.length === 1 ? { repo: repos[0]! } : null);
  const [showClean, setShowClean] = useState(false);

  if (open?.file) return <FileDiff repo={open.repo} file={open.file} onBack={() => setOpen({ repo: open.repo })} />;
  if (open) return <RepoChanges repo={open.repo} status={git[open.repo.id] ?? null} single={repos.length === 1} refresh={refresh} onAsk={onAsk} onFile={(file) => setOpen({ repo: open.repo, file })} onBack={() => setOpen(null)} />;

  const busy = repos.filter((r) => git[r.id] === undefined);
  const dirty = repos.filter((r) => {
    const g = git[r.id];
    return g && (g.files.length || g.ahead || g.behind || g.state);
  });
  const clean = repos.filter((r) => git[r.id] && !dirty.includes(r));
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      {dirty.length ? (
        <ul className="divide-y divide-line">
          {dirty.map((r) => (
            <li key={r.id}>
              <button type="button" onClick={() => setOpen({ repo: r })} className="flex w-full items-center gap-3 px-4 py-3 text-left active:bg-sunken">
                <RepoSummary repo={r} status={git[r.id]!} />
                <ChevronRight className="size-4 shrink-0 text-muted" />
              </button>
            </li>
          ))}
        </ul>
      ) : busy.length ? null : (
        <p className="px-4 py-6 text-center text-sm text-muted">{t('mobile.allClean')}</p>
      )}
      {busy.length ? (
        <div className="flex justify-center py-4">
          <Spinner />
        </div>
      ) : null}
      {clean.length ? (
        <div className="border-t border-line">
          <button type="button" onClick={() => setShowClean(!showClean)} className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm text-muted">
            <ChevronRight className={`size-4 transition-transform ${showClean ? 'rotate-90' : ''}`} />
            {t('mobile.cleanRepos', { n: clean.length })}
          </button>
          {showClean ? (
            <ul className="pb-2">
              {clean.map((r) => (
                <li key={r.id} className="flex items-center gap-2 px-4 py-1.5 pl-10 text-sm">
                  <span className="min-w-0 flex-1 truncate">{r.name}</span>
                  <span className="shrink-0 font-mono text-xs text-muted">{git[r.id]?.branch ?? '—'}</span>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function RepoSummary({ repo, status }: { repo: Repo; status: GitStatus }) {
  return (
    <span className="min-w-0 flex-1">
      <span className="block truncate font-medium">{repo.name}</span>
      <span className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-muted">
        <span className="inline-flex min-w-0 items-center gap-1 font-mono">
          <GitBranch className="size-3 shrink-0" />
          <span className="truncate">{status.branch ?? 'detached'}</span>
        </span>
        {status.ahead ? (
          <span className="inline-flex items-center gap-0.5">
            <ArrowUp className="size-3" />
            {status.ahead}
          </span>
        ) : null}
        {status.behind ? (
          <span className="inline-flex items-center gap-0.5">
            <ArrowDown className="size-3" />
            {status.behind}
          </span>
        ) : null}
        {status.files.length ? <span className="text-accent">{t('mobile.filesChanged', { n: status.files.length })}</span> : null}
      </span>
    </span>
  );
}

/** Letter and colour of a file's change. */
function mark(f: GitFile): { letter: string; cls: string } {
  if (f.conflicted) return { letter: '!', cls: 'text-danger' };
  if (f.untracked) return { letter: 'U', cls: 'text-ok' };
  const x = f.worktree !== ' ' ? f.worktree : f.index;
  if (x === 'A') return { letter: 'A', cls: 'text-ok' };
  if (x === 'D') return { letter: 'D', cls: 'text-danger' };
  if (x === 'R') return { letter: 'R', cls: 'text-warn' };
  return { letter: 'M', cls: 'text-warn' };
}

function RepoChanges({
  repo,
  status,
  single,
  refresh,
  onAsk,
  onFile,
  onBack,
}: {
  repo: Repo;
  status: GitStatus | null;
  single: boolean;
  refresh: () => void;
  onAsk: (text: string) => void;
  onFile: (f: GitFile) => void;
  onBack: () => void;
}) {
  const [busy, setBusy] = useState<'pull' | 'push' | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [auth, setAuth] = useState<GitAuthProblem | null>(null);
  const [login, setLogin] = useState(false);

  const op = async (name: 'pull' | 'push') => {
    setBusy(name);
    setResult(null);
    setAuth(null);
    try {
      const r = await api.gitOp(repo.id, name);
      if (r.auth) setAuth(r.auth);
      setResult({ ok: r.ok, text: r.ok ? t(name === 'pull' ? 'mobile.pulled' : 'mobile.pushed') : r.output.trim().split('\n').slice(-6).join('\n') });
      refresh();
    } catch (e) {
      setResult({ ok: false, text: (e as Error).message });
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {single ? null : (
        <button type="button" onClick={onBack} className="flex shrink-0 items-center gap-1 border-b border-line px-2 py-2 text-sm text-muted">
          <ChevronLeft className="size-4" /> {t('mobile.allRepos')}
        </button>
      )}
      <div className="min-h-0 flex-1 overflow-y-auto">
        {status ? (
          <>
            <div className="flex items-center gap-2 px-4 py-3">
              <RepoSummary repo={repo} status={status} />
              <button type="button" onClick={refresh} aria-label={t('files.refresh')} className="tap inline-flex size-9 items-center justify-center text-muted">
                <RefreshCw className="size-4" />
              </button>
            </div>
            {status.files.length ? (
              <ul className="divide-y divide-line border-y border-line">
                {status.files.map((f) => {
                  const m = mark(f);
                  const slash = f.path.lastIndexOf('/');
                  return (
                    <li key={f.path}>
                      <button type="button" onClick={() => onFile(f)} className="flex w-full items-center gap-3 px-4 py-2.5 text-left active:bg-sunken">
                        <span className={`w-3 shrink-0 font-mono text-xs font-semibold ${m.cls}`}>{m.letter}</span>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-sm">{f.path.slice(slash + 1)}</span>
                          {slash > 0 ? <span className="block truncate text-xs text-muted">{f.path.slice(0, slash)}</span> : null}
                        </span>
                        <ChevronRight className="size-4 shrink-0 text-muted" />
                      </button>
                    </li>
                  );
                })}
              </ul>
            ) : (
              <p className="px-4 py-3 text-sm text-muted">{t('mobile.noChanges')}</p>
            )}
            <div className="space-y-2 p-4">
              {status.files.length || status.ahead ? (
                <Button variant="primary" className="w-full justify-center" onClick={() => onAsk(t('mobile.commitPrompt', { repo: repo.name }))}>
                  <Bot className="size-4" /> {t('mobile.askCommit')}
                </Button>
              ) : null}
              <div className="grid grid-cols-2 gap-2">
                <Button className="justify-center" disabled={busy !== null} onClick={() => void op('pull')}>
                  {busy === 'pull' ? <Loader2 className="size-4 animate-spin" /> : <ArrowDown className="size-4" />} Pull
                </Button>
                <Button className="justify-center" disabled={busy !== null} onClick={() => void op('push')}>
                  {busy === 'push' ? <Loader2 className="size-4 animate-spin" /> : <ArrowUp className="size-4" />} Push
                </Button>
              </div>
              {auth ? <GitAuthNotice problem={auth} onSignIn={() => setLogin(true)} /> : null}
              {result ? (
                result.ok ? (
                  <p className="text-sm text-ok">{result.text}</p>
                ) : (
                  <pre className="overflow-x-auto bg-bg p-2 font-mono text-xs whitespace-pre-wrap text-danger">{result.text}</pre>
                )
              ) : null}
            </div>
          </>
        ) : (
          <ErrorLine error={t('mobile.noGit')} />
        )}
      </div>
      <GitLoginSheet
        open={login}
        host={auth?.host}
        onClose={() => setLogin(false)}
        onSaved={() => {
          setLogin(false);
          setAuth(null);
        }}
      />
    </div>
  );
}

function FileDiff({ repo, file, onBack }: { repo: Repo; file: GitFile; onBack: () => void }) {
  const [diff, setDiff] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    // the working-tree change, else what is staged (a file only in the index), new files whole
    const staged = !file.untracked && file.worktree === ' ' && file.index !== ' ';
    api.gitDiff(repo.id, { path: file.path, staged, untracked: file.untracked }).then(
      (r) => setDiff(r.diff),
      (e) => setError((e as Error).message),
    );
  }, [repo.id, file]);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <button type="button" onClick={onBack} className="flex shrink-0 items-center gap-1 border-b border-line px-2 py-2 text-left text-sm">
        <ChevronLeft className="size-4 shrink-0 text-muted" />
        <span className="min-w-0 truncate font-mono text-xs">{file.path}</span>
      </button>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {error ? <ErrorLine error={error} /> : diff === null ? <Spinner /> : <DiffView diff={diff} path={file.path} wrap />}
      </div>
    </div>
  );
}

// ---------- Files ----------

function FilesTab({ repos, git }: { repos: Repo[]; git: Record<string, GitStatus | null> }) {
  const [repo, setRepo] = useState<Repo | null>(repos.length === 1 ? repos[0]! : null);
  const [dir, setDir] = useState('');
  const [file, setFile] = useState<string | null>(null);
  const [entries, setEntries] = useState<FsEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!repo) return;
    setEntries(null);
    setError(null);
    api.tree(repo.id, dir).then(
      (list) => setEntries([...list].sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name))),
      (e) => setError((e as Error).message),
    );
  }, [repo, dir]);

  if (!repo)
    return (
      <ul className="min-h-0 flex-1 divide-y divide-line overflow-y-auto">
        {repos.map((r) => (
          <li key={r.id}>
            <button type="button" onClick={() => setRepo(r)} className="flex w-full items-center gap-3 px-4 py-3 text-left active:bg-sunken">
              <Folder className="size-4 shrink-0 text-accent" />
              <span className="min-w-0 flex-1 truncate">{r.name}</span>
              <ChevronRight className="size-4 shrink-0 text-muted" />
            </button>
          </li>
        ))}
      </ul>
    );

  if (file)
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <FileViewer projectId={repo.id} rootName={repos.length > 1 ? repo.name : undefined} path={file} git={git[repo.id] ?? null} onBack={() => setFile(null)} onCrumb={(d) => (setFile(null), setDir(d))} />
      </div>
    );

  const up = dir ? dir.slice(0, Math.max(0, dir.lastIndexOf('/'))) : null;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <button
        type="button"
        onClick={() => (up !== null ? setDir(up) : repos.length > 1 ? setRepo(null) : undefined)}
        className="flex shrink-0 items-center gap-1 border-b border-line px-2 py-2 text-left text-sm"
      >
        {up !== null || repos.length > 1 ? <ChevronLeft className="size-4 shrink-0 text-muted" /> : <span className="w-1" />}
        <span className="min-w-0 truncate font-mono text-xs">
          {repo.name}
          {dir ? `/${dir}` : ''}
        </span>
      </button>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {error ? (
          <ErrorLine error={error} />
        ) : entries === null ? (
          <div className="flex justify-center py-6">
            <Spinner />
          </div>
        ) : (
          <ul className="divide-y divide-line">
            {entries.map((e) => (
              <li key={e.path}>
                <button
                  type="button"
                  onClick={() => (e.dir ? setDir(e.path) : setFile(e.path))}
                  className={`flex w-full items-center gap-3 px-4 py-2.5 text-left active:bg-sunken ${e.ignored ? 'opacity-50' : ''}`}
                >
                  {e.dir ? <Folder className="size-4 shrink-0 text-accent" /> : <File className="size-4 shrink-0 text-muted" />}
                  <span className="min-w-0 flex-1 truncate text-sm">{e.name}</span>
                  {e.dir ? <ChevronRight className="size-4 shrink-0 text-muted" /> : null}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

// ---------- More ----------

function MoreTab({ tools, onFull }: { tools: Tool[]; onFull: () => void }) {
  const [open, setOpen] = useState<string | null>(null);
  const tool = tools.find((x) => x.id === open);
  if (tool)
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <button type="button" onClick={() => setOpen(null)} className="flex shrink-0 items-center gap-1 border-b border-line px-2 py-2 text-sm text-muted">
          <ChevronLeft className="size-4" /> {tool.label}
        </button>
        <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">{tool.render()}</div>
      </div>
    );
  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <ul className="divide-y divide-line">
        {tools.map((x) => (
          <li key={x.id}>
            <button type="button" onClick={() => setOpen(x.id)} className="flex w-full items-center gap-3 px-4 py-3 text-left active:bg-sunken">
              <x.icon className="size-4 shrink-0 text-accent" />
              <span className="min-w-0 flex-1 truncate">{x.label}</span>
              {x.badge === 'dot' ? <span className="size-2 rounded-full bg-ok" /> : null}
              <ChevronRight className="size-4 shrink-0 text-muted" />
            </button>
          </li>
        ))}
      </ul>
      <div className="p-4">
        <Button className="w-full justify-center" onClick={onFull}>
          <LayoutPanelLeft className="size-4" /> {t('mobile.fullWorkspace')}
        </Button>
        <p className="mt-2 text-center text-xs text-muted">{t('mobile.fullWorkspaceHint')}</p>
      </div>
    </div>
  );
}

/** In the full workspace on a phone: back to the simple view. */
export function SimpleViewButton({ onClick }: { onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} aria-label={t('mobile.simpleView')} title={t('mobile.simpleView')} className="tap inline-flex size-9 items-center justify-center text-muted hover:text-fg">
      <Smartphone className="size-4" />
    </button>
  );
}
