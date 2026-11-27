import { gitMark } from '../lib/gitMark';
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { ArrowDown, ArrowUp, Check, ChevronRight, CloudUpload, GitBranch, Loader2, Minus, Plus, RefreshCw, RotateCcw, Sparkles, X } from 'lucide-react';
import type { GitCommit, GitFile, GitOpResult } from '@hubness/shared';
import { useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/api';
import { useApi } from '../lib/query';
import { useAsk } from '../lib/ask';
import { useRepo } from '../lib/repo';
import { useWorkbench } from '../lib/workbench';
import { splitPath, timeAgo } from '../lib/format';
import { Button, Empty, ErrorLine, inputCls, Spinner } from '../components/ui';
import { confirmDialog } from '../components/Confirm';
import { DiffView } from '../components/DiffView';
import { FileIcon } from '../components/FileIcon';
import { GitAuthNotice, GitLoginSheet } from '../components/GitLogin';
import { Select } from '../components/Select';
import { t } from '../i18n';
import { discard, switchBranch } from './git/actions';
import { BranchDetail, BranchLine } from './git/branches';
import { authorColor, CommitDetails, RefTag, StashDetail } from './git/history';
import { type Ctx, type OpResult, type Run } from './git/types';

/**
 * Git as a source-control panel (the lazygit-style screen lives on in the TUI): the branch
 * and one sync button on top, a commit box, staged and unstaged changes with per-file
 * actions (a click opens the diff in the editor), then history, branches and stashes as
 * sections that fold. Every action is a visible button; nothing needs a key.
 */
export function GitPage() {
  const { repo, git, refreshGit } = useRepo();
  const wb = useWorkbench();
  const ask = useAsk();
  const queryClient = useQueryClient();
  const [op, setOp] = useState<string | null>(null);
  const [result, setResult] = useState<OpResult | null>(null);
  const [login, setLogin] = useState<string | null>(null);
  const [logLimit, setLogLimit] = useState(50);
  const [inlineDiff, setInlineDiff] = useState<string | null>(null);

  const branches = useApi(['git', repo.id, 'branches'], () => api.gitBranches(repo.id));
  // "load more" is another query: the commits already shown stay while it loads (of this repository only)
  const log = useApi(['git', repo.id, 'log', logLimit, false], () => api.gitLog(repo.id, { limit: logLimit, all: false }), { keepWhile: (prev) => prev[1] === repo.id });
  const stashes = useApi(['git', repo.id, 'stashes'], () => api.gitStashes(repo.id));

  // a success fades by itself; an error stays until dismissed
  useEffect(() => {
    if (!result?.ok) return;
    const timer = setTimeout(() => setResult(null), 4000);
    return () => clearTimeout(timer);
  }, [result]);

  const run: Run = async (name, label, body) => {
    setOp(name);
    let r: GitOpResult;
    try {
      r = await api.gitOp(repo.id, name, body);
    } catch (e) {
      r = { ok: false, output: (e as Error).message };
    } finally {
      setOp(null);
      refreshGit();
      void queryClient.invalidateQueries({ queryKey: ['git', repo.id] });
    }
    setResult({ ...r, name: label, retry: () => void run(name, label, body) });
    return r;
  };

  const files = git?.files ?? [];
  const staged = files.filter((f) => f.index !== ' ' && !f.untracked && !f.conflicted);
  const unstaged = files.filter((f) => f.worktree !== ' ' || f.untracked || f.conflicted);
  const local = useMemo(() => (branches.data ?? []).filter((b) => !b.remote), [branches.data]);
  const remote = useMemo(() => (branches.data ?? []).filter((b) => b.remote), [branches.data]);

  if (!git)
    return (
      <div className="flex justify-center py-10">
        <Spinner />
      </div>
    );
  if (!git.isRepo) return <Empty title={t('git.notRepo')} hint={repo.path} />;

  const ctx: Ctx = { projectId: repo.id, git, run, busy: !!op };

  // one button: publish a branch without upstream, else pull what is behind and push what is ahead
  const sync = async () => {
    if (!git.upstream) return void run('push', t('git.publish'));
    if (git.behind && !(await run('pull', t('git.pull'))).ok) return;
    if (git.ahead) await run('push', t('git.push'));
    if (!git.behind && !git.ahead) await run('fetch', t('git.fetch'));
  };
  const syncLabel = !git.upstream ? t('git.publish') : git.behind || git.ahead ? t('git.sync') : t('git.fetch');

  const openDiff = (f: GitFile, isStaged: boolean) => {
    if (wb && !f.untracked && !isStaged) return wb.openFile(repo.id, f.path, { mode: 'diff' });
    const key = `${isStaged ? 's' : 'w'}:${f.path}`;
    setInlineDiff(inlineDiff === key ? null : key);
  };

  const stateName = git.state ? t(`git.state.${git.state}` as 'git.state.merge') : null;
  const conflicts = files.filter((f) => f.conflicted).length;
  const commits = log.data?.commits ?? [];

  return (
    <div className="flex min-h-0 w-full flex-col text-[13px]">
      {/* branch and sync */}
      <div className="flex items-center gap-2 border-b border-line px-3 py-2">
        <GitBranch className="size-4 shrink-0 text-accent" />
        <div className="min-w-0 flex-1">
          <Select
            label={t('git.switchBranch')}
            size="sm"
            value={git.branch ?? ''}
            onChange={(b) => b !== git.branch && void switchBranch(run, b)}
            options={[
              ...(git.branch ? [] : [{ value: '', label: t('gitline.detached') }]),
              ...local.map((b) => ({ value: b.name, label: b.name })),
              ...remote.filter((r) => !local.some((l) => r.name.endsWith(`/${l.name}`))).map((b) => ({ value: b.name.replace(/^[^/]+\//, ''), label: b.name })),
            ]}
          />
        </div>
        <Button size="sm" disabled={!!op} onClick={() => void sync()} title={git.upstream ?? t('git.noUpstream')}>
          {op && ['pull', 'push', 'fetch'].includes(op) ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : !git.upstream ? (
            <CloudUpload className="size-3.5" />
          ) : (
            <RefreshCw className="size-3.5" />
          )}
          <span>{syncLabel}</span>
          {git.behind ? (
            <span className="inline-flex items-center text-warn">
              <ArrowDown className="size-3" />
              {git.behind}
            </span>
          ) : null}
          {git.ahead ? (
            <span className="inline-flex items-center text-ok">
              <ArrowUp className="size-3" />
              {git.ahead}
            </span>
          ) : null}
        </Button>
      </div>

      {/* merge / rebase in progress */}
      {stateName ? (
        <div className="space-y-2 border-b border-warn/40 bg-warn/10 px-3 py-2">
          <div className="font-medium text-warn">{stateName}</div>
          <div className="text-xs text-muted">{conflicts ? t('git.conflicts', { n: conflicts }) : t('git.readyToContinue')}</div>
          <div className="flex gap-2">
            <Button size="sm" variant="primary" disabled={!!op || conflicts > 0} onClick={() => void run('continue', t('git.continue'))}>
              {t('git.continue')}
            </Button>
            <Button
              size="sm"
              variant="danger"
              disabled={!!op}
              onClick={async () => {
                if (await confirmDialog({ message: t('git.abortConfirm', { op: stateName }), confirmLabel: t('git.abort'), danger: true })) void run('abort', t('git.abort'));
              }}
            >
              {t('git.abort')}
            </Button>
          </div>
        </div>
      ) : null}

      {/* the last operation */}
      {result ? (
        <div className={`border-b px-3 py-2 text-xs ${result.ok ? 'border-line text-ok' : 'border-danger/40 bg-danger/5 text-danger'}`}>
          <div className="flex items-center gap-2">
            {result.ok ? <Check className="size-3.5 shrink-0" /> : <X className="size-3.5 shrink-0" />}
            <span className="min-w-0 flex-1 truncate">{result.name}</span>
            <button type="button" onClick={() => setResult(null)} aria-label={t('common.close')} className="text-muted hover:text-fg">
              <X className="size-3.5" />
            </button>
          </div>
          {result.auth ? <GitAuthNotice problem={result.auth} onSignIn={() => setLogin(result.auth!.host)} /> : null}
          {!result.ok && result.output ? <pre className="mt-1 max-h-32 overflow-auto font-mono whitespace-pre-wrap">{result.output}</pre> : null}
        </div>
      ) : null}

      <CommitBox ctx={ctx} stagedCount={staged.length} unstagedCount={unstaged.length} onAsk={ask ? () => ask.ask(t('git.askAgentPrompt', { repo: repo.name }), 'new') : undefined} />

      {/* changes */}
      {staged.length ? (
        <Section
          title={t('git.staged')}
          count={staged.length}
          action={
            <IconAct label={t('git.unstageAll')} disabled={!!op} onClick={() => void run('unstage', t('git.unstage'), { paths: staged.map((f) => f.path) })}>
              <Minus className="size-3.5" />
            </IconAct>
          }
        >
          {staged.map((f) => (
            <FileRow key={`s:${f.path}`} f={f} staged ctx={ctx} onOpen={() => openDiff(f, true)} inline={inlineDiff === `s:${f.path}`} />
          ))}
        </Section>
      ) : null}
      {unstaged.length ? (
        <Section
          title={t('git.changes')}
          count={unstaged.length}
          action={
            <IconAct label={t('git.stageAll')} disabled={!!op} onClick={() => void run('stage', t('git.stage'), { paths: unstaged.map((f) => f.path) })}>
              <Plus className="size-3.5" />
            </IconAct>
          }
        >
          {unstaged.map((f) => (
            <FileRow key={`w:${f.path}`} f={f} staged={false} ctx={ctx} onOpen={() => openDiff(f, false)} inline={inlineDiff === `w:${f.path}`} />
          ))}
        </Section>
      ) : null}
      {!files.length ? <p className="px-3 py-3 text-muted">{t('git.clean')}</p> : null}

      {/* history, branches, stashes */}
      <Section title={t('git.history')} defaultOpen>
        {log.error ? <ErrorLine error={log.error} /> : null}
        {log.loading && !log.data ? <Spinner /> : null}
        {commits.length ? commits.map((c) => <CommitRow key={c.hash} c={c} ctx={ctx} />) : log.data ? <p className="px-3 py-2 text-muted">{t('git.noCommits')}</p> : null}
        {log.data?.more ? (
          <button type="button" onClick={() => setLogLimit((n) => n + 100)} className="w-full px-3 py-2 text-left text-xs text-accent hover:bg-sunken">
            {t('git.loadMore')}
          </button>
        ) : null}
      </Section>
      <Section title={t('git.branches')} count={local.length}>
        <NewBranchRow ctx={ctx} />
        {[...local, ...remote].map((b) => (
          <Fold key={b.name} head={<BranchLine b={b} />}>
            <BranchDetail ctx={ctx} b={b} />
          </Fold>
        ))}
      </Section>
      {stashes.data?.length ? (
        <Section title={t('git.stash')} count={stashes.data.length}>
          {stashes.data.map((s) => (
            <Fold
              key={s.ref}
              head={
                <span className="flex min-w-0 gap-2">
                  <span className="min-w-0 flex-1 truncate">{s.message}</span>
                  <span className="shrink-0 text-xs text-muted">{s.date}</span>
                </span>
              }
            >
              <StashDetail ctx={ctx} s={s} />
            </Fold>
          ))}
        </Section>
      ) : null}

      <GitLoginSheet
        open={login !== null}
        host={login ?? undefined}
        saveLabel={t('gitauth.saveRetry')}
        onClose={() => setLogin(null)}
        onSaved={() => {
          setLogin(null);
          result?.retry();
        }}
      />
    </div>
  );
}

/** A section that folds, with a count and an action on the right of its header. */
function Section({ title, count, action, defaultOpen = true, children }: { title: string; count?: number; action?: ReactNode; defaultOpen?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section className="border-b border-line">
      <div className="flex items-center gap-1 pr-2">
        <button type="button" onClick={() => setOpen(!open)} className="flex min-w-0 flex-1 items-center gap-1.5 px-3 py-2 text-left text-xs font-semibold tracking-wide text-muted uppercase hover:text-fg">
          <ChevronRight className={`size-3.5 shrink-0 transition-transform ${open ? 'rotate-90' : ''}`} />
          <span className="truncate">{title}</span>
          {count !== undefined ? <span className="bg-sunken px-1.5 font-normal tracking-normal normal-case">{count}</span> : null}
        </button>
        {action}
      </div>
      {open ? <div className="pb-1">{children}</div> : null}
    </section>
  );
}

function IconAct({ label, onClick, disabled, danger, children }: { label: string; onClick: () => void; disabled?: boolean; danger?: boolean; children: ReactNode }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      disabled={disabled}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      className={`inline-flex size-7 shrink-0 items-center justify-center text-muted disabled:opacity-40 ${danger ? 'hover:text-danger' : 'hover:text-fg'} hover:bg-sunken`}
    >
      {children}
    </button>
  );
}

function FileRow({ f, staged, ctx, onOpen, inline }: { f: GitFile; staged: boolean; ctx: Ctx; onOpen: () => void; inline: boolean }) {
  const { dir, name } = splitPath(f.path);
  const m = gitMark(f, staged ? 'staged' : 'worktree');
  return (
    <div>
      <div className="group flex items-center gap-2 pr-2 pl-5 hover:bg-sunken">
        <button type="button" onClick={onOpen} className="flex min-w-0 flex-1 items-center gap-2 py-1 text-left" title={f.path}>
          <FileIcon name={name} className="size-4 shrink-0" />
          <span className="min-w-0 truncate">
            <span className={f.worktree === 'D' || f.index === 'D' ? 'line-through' : ''}>{name}</span>
            {dir ? <span className="ml-2 text-xs text-muted">{dir.replace(/\/$/, '')}</span> : null}
          </span>
        </button>
        {/* actions show on hover with a mouse, always on touch */}
        <span className="flex items-center opacity-100 pointer-fine:opacity-0 pointer-fine:group-hover:opacity-100">
          {staged ? (
            <IconAct label={t('git.unstage')} disabled={ctx.busy} onClick={() => void ctx.run('unstage', t('git.unstage'), { paths: [f.path] })}>
              <Minus className="size-3.5" />
            </IconAct>
          ) : (
            <>
              {f.conflicted ? null : (
                <IconAct label={t('git.discard')} danger disabled={ctx.busy} onClick={() => void discard(ctx.run, [f.path], f.path)}>
                  <RotateCcw className="size-3.5" />
                </IconAct>
              )}
              <IconAct label={t('git.stage')} disabled={ctx.busy} onClick={() => void ctx.run('stage', t('git.stage'), { paths: [f.path] })}>
                <Plus className="size-3.5" />
              </IconAct>
            </>
          )}
        </span>
        <span className={`w-3 shrink-0 text-center font-mono text-xs font-semibold ${m.cls}`}>{m.letter}</span>
      </div>
      {inline ? <InlineDiff ctx={ctx} f={f} staged={staged} /> : null}
    </div>
  );
}

function InlineDiff({ ctx, f, staged }: { ctx: Ctx; f: GitFile; staged: boolean }) {
  const { data, error } = useApi(['git', ctx.projectId, 'diff', f.path, staged, f.untracked, ctx.git], () =>
    api.gitDiff(ctx.projectId, { path: f.path, staged, untracked: f.untracked }),
  );
  return <div className="border-y border-line bg-bg">{error ? <ErrorLine error={error} /> : !data ? <Spinner /> : <DiffView diff={data.diff} maxHeight="50dvh" path={f.path} />}</div>;
}

function CommitBox({ ctx, stagedCount, unstagedCount, onAsk }: { ctx: Ctx; stagedCount: number; unstagedCount: number; onAsk?: () => void }) {
  const [message, setMessage] = useState('');
  if (!stagedCount && !unstagedCount) return null;
  // nothing staged: commit everything, the way most people expect
  const all = stagedCount === 0;
  const commit = async (push: boolean) => {
    if (!message.trim()) return;
    if (!(await ctx.run('commit', t('git.commit'), { message, all })).ok) return;
    setMessage('');
    if (push) await ctx.run('push', t('git.push'));
  };
  return (
    <div className="space-y-2 border-b border-line px-3 py-3">
      <textarea
        rows={2}
        className={`${inputCls} resize-y`}
        placeholder={t('git.commitMessage')}
        value={message}
        onChange={(e) => setMessage(e.target.value)}
        onKeyDown={(e) => {
          // like the buttons: not on top of a pull / push / commit that is still running
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && !ctx.busy) void commit(false);
        }}
      />
      {/* stacked: the side bar is narrow */}
      <Button size="sm" variant="primary" className="w-full justify-center" disabled={!message.trim() || ctx.busy} onClick={() => void commit(false)}>
        <Check className="size-3.5 shrink-0" />
        <span className="truncate">{all ? t('git.commitAll', { n: unstagedCount }) : t('git.commitStaged', { n: stagedCount })}</span>
      </Button>
      <Button size="sm" className="w-full justify-center" disabled={!message.trim() || ctx.busy} onClick={() => void commit(true)}>
        <ArrowUp className="size-3.5 shrink-0" />
        <span className="truncate">{t('git.commitPush')}</span>
      </Button>
      {onAsk ? (
        <button type="button" onClick={onAsk} className="flex w-full items-center justify-center gap-1.5 py-0.5 text-xs text-accent hover:underline">
          <Sparkles className="size-3.5 shrink-0" />
          <span className="truncate">{t('git.askAgent')}</span>
        </button>
      ) : null}
    </div>
  );
}

/** A row that unfolds its details under it. */
function Fold({ head, children }: { head: ReactNode; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button type="button" onClick={() => setOpen(!open)} className={`flex w-full min-w-0 items-center gap-2 py-1 pr-3 pl-5 text-left hover:bg-sunken ${open ? 'bg-sunken' : ''}`}>
        <ChevronRight className={`size-3 shrink-0 text-muted transition-transform ${open ? 'rotate-90' : ''}`} />
        <span className="min-w-0 flex-1">{head}</span>
      </button>
      {open ? <div className="px-3">{children}</div> : null}
    </div>
  );
}

function CommitRow({ c, ctx }: { c: GitCommit; ctx: Ctx }) {
  const initials = c.author
    .split(/\s+/)
    .map((w) => w[0])
    .join('')
    .slice(0, 2)
    .toUpperCase();
  return (
    <Fold
      head={
        <span className="flex min-w-0 items-center gap-2">
          <span className="inline-flex size-5 shrink-0 items-center justify-center rounded-full text-[9px] font-semibold text-bg" style={{ background: authorColor(c.author) }} title={c.author}>
            {initials}
          </span>
          <span className="min-w-0 flex-1">
            <span className="block truncate">
              {c.subject}
              {c.refs.length ? (
                <span className="ml-2 inline-flex gap-1 align-middle">
                  {c.refs.slice(0, 2).map((r) => (
                    <RefTag key={r} name={r} />
                  ))}
                </span>
              ) : null}
            </span>
            <span className="block truncate text-xs text-muted">
              {c.author} · {relative(c.date)}
            </span>
          </span>
        </span>
      }
    >
      <CommitDetails ctx={ctx} c={c} />
    </Fold>
  );
}

/** "2 h ago" when the date parses, the date as given otherwise. */
function relative(date: string) {
  const ms = Date.parse(date);
  return Number.isNaN(ms) ? date : timeAgo(new Date(ms).toISOString());
}

function NewBranchRow({ ctx }: { ctx: Ctx }) {
  const [name, setName] = useState('');
  return (
    <form
      className="flex gap-2 px-3 py-1.5"
      onSubmit={async (e) => {
        e.preventDefault();
        const n = name.trim();
        if (n && (await ctx.run('checkout', t('git.switching', { branch: n }), { branch: n, create: true })).ok) setName('');
      }}
    >
      <input className={`${inputCls} h-8 py-0`} placeholder={t('git.newBranch')} value={name} onChange={(e) => setName(e.target.value)} autoCapitalize="off" spellCheck={false} />
      <Button type="submit" size="sm" disabled={!name.trim() || ctx.busy}>
        <Plus className="size-3.5" /> {t('git.create')}
      </Button>
    </form>
  );
}
