import { useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import type { GitOpResult } from '@hubness/shared';
import { api } from '../lib/api';
import { useQueryClient } from '@tanstack/react-query';
import { useApi } from '../lib/query';
import { Empty, Spinner } from '../components/ui';
import { useRepo } from '../lib/repo';
import { t } from '../i18n';
import { deleteBranch, discard, dropStash, mergeBranch, rebaseBranch, switchBranch } from './git/actions';
import { BranchDetail, BranchLine, NewBranch } from './git/branches';
import { CommitDetails, CommitList, StashDetail } from './git/history';
import { CommitBox, FileDetail, FileLine, StatusPanel } from './git/status';
import { type Ctx, type OpResult, type PanelId, type Run, fileKey } from './git/types';
import { Busy, KeyHints, Panel, Row } from './git/ui';

/**
 * Git in the spirit of lazygit: numbered panels ([1] Status … [5] Stash) with dense
 * one-line rows and a selection. Wide: panels on the left, the main view (diff, commit,
 * actions) on the right. Narrow: accordion — the focused panel is open, details unfold
 * under the selected row. Keys work while the page has focus (1-5, j/k, space, …).
 */


const WIDE_PX = 860;

function useWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setW(e.contentRect.width));
    ro.observe(el);
    setW(el.getBoundingClientRect().width);
    return () => ro.disconnect();
  }, []);
  return [ref, w] as const;
}


export function GitPage() {
  const { repo, git, refreshGit } = useRepo();
  const [rootRef, width] = useWidth<HTMLDivElement>();
  const wide = width >= WIDE_PX;
  const [focus, setFocus] = useState<PanelId>(2);
  const [op, setOp] = useState<string | null>(null);
  const [result, setResult] = useState<OpResult | null>(null);
  // selection per panel
  const [selFile, setSelFile] = useState<string | null>(null);
  const [selBranch, setSelBranch] = useState<string | null>(null);
  const [selCommit, setSelCommit] = useState<string | null>(null);
  const [selStash, setSelStash] = useState<string | null>(null);
  // narrow: the selected row unfolds only after a tap / Enter
  const [unfold, setUnfold] = useState(false);
  const [logAll, setLogAll] = useState(true);
  const [logLimit, setLogLimit] = useState(150);
  const commitInput = useRef<HTMLTextAreaElement>(null);

  const queryClient = useQueryClient();
  const branches = useApi(['git', repo.id, 'branches'], () => api.gitBranches(repo.id));
  const log = useApi(['git', repo.id, 'log', logLimit, logAll], () => api.gitLog(repo.id, { limit: logLimit, all: logAll }));
  const stashes = useApi(['git', repo.id, 'stashes'], () => api.gitStashes(repo.id));

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
      // an operation may change branches, history, stashes and diffs: re-read this repository's git data
      void queryClient.invalidateQueries({ queryKey: ['git', repo.id] });
    }
    setResult({ ...r, name: label, retry: () => void run(name, label, body) });
    return r;
  };

  const files = git?.files ?? [];
  const staged = files.filter((f) => f.index !== ' ' && !f.untracked && !f.conflicted);
  const unstaged = files.filter((f) => f.worktree !== ' ' || f.untracked || f.conflicted);
  const fileRows = [...unstaged.map((f) => ({ f, staged: false })), ...staged.map((f) => ({ f, staged: true }))];
  const branchRows = useMemo(() => [...(branches.data ?? []).filter((b) => !b.remote), ...(branches.data ?? []).filter((b) => b.remote)], [branches.data]);
  const commits = log.data?.commits ?? [];
  const stashRows = stashes.data ?? [];

  if (!git) return <div className="flex justify-center py-10"><Spinner /></div>;
  if (!git.isRepo) return <Empty title={t('git.notRepo')} hint={repo.path} />;

  const ctx: Ctx = { projectId: repo.id, git, run, busy: !!op };

  // ----- keyboard -----
  const move = (d: number) => {
    const step = <T,>(list: T[], key: (x: T) => string, cur: string | null, set: (k: string) => void) => {
      if (!list.length) return;
      const i = list.findIndex((x) => key(x) === cur);
      const n = Math.max(0, Math.min(list.length - 1, (i < 0 ? -1 : i) + d));
      set(key(list[n]));
    };
    setUnfold(false);
    if (focus === 2) step(fileRows, (r) => fileKey(r.f, r.staged), selFile, setSelFile);
    if (focus === 3) step(branchRows, (b) => b.name, selBranch, setSelBranch);
    if (focus === 4) step(commits, (c) => c.hash, selCommit, setSelCommit);
    if (focus === 5) step(stashRows, (s) => s.ref, selStash, setSelStash);
  };
  const selectedFile = fileRows.find((r) => fileKey(r.f, r.staged) === selFile);
  const selectedBranch = branchRows.find((b) => b.name === selBranch);
  const selectedStash = stashRows.find((s) => s.ref === selStash);

  const onKey = (e: ReactKeyboardEvent) => {
    const tag = (e.target as HTMLElement).tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || e.metaKey || e.ctrlKey || e.altKey) return;
    // a dialog opened from a panel (sign-in) keeps its keys
    if ((e.target as HTMLElement).closest('[role=dialog]')) return;
    const k = e.key;
    const handled = () => e.preventDefault();
    if (k >= '1' && k <= '5') return handled(), setFocus(Number(k) as PanelId);
    if (k === 'j' || k === 'ArrowDown') return handled(), move(1);
    if (k === 'k' || k === 'ArrowUp') return handled(), move(-1);
    if (k === 'Enter') return handled(), setUnfold((v) => !v);
    if (k === 'f') return handled(), void run('fetch', 'Fetch');
    if (k === 'p') return handled(), void run('pull', 'Pull');
    if (k === 'P') return handled(), void run('push', 'Push');
    if (focus === 2) {
      if (k === ' ' && selectedFile) return handled(), void run(selectedFile.staged ? 'unstage' : 'stage', selectedFile.staged ? t('git.unstage') : t('git.stage'), { paths: [selectedFile.f.path] });
      if (k === 'a' && files.length) return handled(), void (unstaged.length ? run('stage', t('git.stage'), { paths: unstaged.map((f) => f.path) }) : run('unstage', t('git.unstage'), { paths: staged.map((f) => f.path) }));
      if (k === 'd' && selectedFile) return handled(), void discard(run, [selectedFile.f.path], selectedFile.f.path);
      if (k === 'c') return handled(), commitInput.current?.focus();
      if (k === 's' && files.length) return handled(), void run('stash-push', t('git.stashPush'), { message: '', untracked: true });
    }
    if (focus === 3 && selectedBranch && !selectedBranch.current) {
      if (k === ' ') return handled(), void switchBranch(run, selectedBranch.name);
      if (k === 'M' && git.branch) return handled(), void mergeBranch(run, selectedBranch.name, git.branch);
      if (k === 'r' && git.branch) return handled(), void rebaseBranch(run, git.branch, selectedBranch.name);
      if (k === 'd' && !selectedBranch.remote) return handled(), void deleteBranch(run, selectedBranch.name);
    }
    if (focus === 5 && selectedStash) {
      if (k === ' ') return handled(), void run('stash', `Stash apply ${selectedStash.ref}`, { action: 'apply', rev: selectedStash.ref });
      if (k === 'g') return handled(), void run('stash', `Stash pop ${selectedStash.ref}`, { action: 'pop', rev: selectedStash.ref });
      if (k === 'd') return handled(), void dropStash(run, selectedStash.ref);
    }
  };

  const select = (panel: PanelId, set: () => void, same: boolean) => {
    setFocus(panel);
    set();
    setUnfold(wide ? false : same ? !unfold : true);
  };

  // ----- panels -----
  const filesPanel = (
    <Panel n={2} title={t('git.files')} focus={focus} wide={wide} setFocus={setFocus} count={files.length} grow summary={files.length ? fileRows.slice(0, 3).map((r) => r.f.path.split('/').pop()).join(', ') : t('git.clean')}>
      {files.length ? (
        <ul>
          {fileRows.map(({ f, staged: st }) => {
            const key = fileKey(f, st);
            const sel = selFile === key && focus === 2;
            return (
              <Row key={key} selected={selFile === key} active={focus === 2} onClick={() => select(2, () => setSelFile(key), selFile === key)}>
                <FileLine f={f} staged={st} />
                {sel && unfold && !wide ? <FileDetail ctx={ctx} f={f} staged={st} /> : null}
              </Row>
            );
          })}
        </ul>
      ) : (
        <div className="px-3 py-2 text-[13px] text-muted">{t('git.clean')}</div>
      )}
      <CommitBox ctx={ctx} inputRef={commitInput} stagedCount={staged.length} unstagedCount={unstaged.length} />
    </Panel>
  );

  const branchesPanel = (
    <Panel n={3} title={t('git.branches')} focus={focus} wide={wide} setFocus={setFocus} count={branchRows.filter((b) => !b.remote).length} summary={`* ${git.branch ?? 'HEAD'}`}>
      <NewBranch ctx={ctx} />
      {branches.loading && !branches.data ? <Busy /> : null}
      <ul>
        {branchRows.map((b) => (
          <Row key={b.name} selected={selBranch === b.name} active={focus === 3} onClick={() => select(3, () => setSelBranch(b.name), selBranch === b.name)}>
            <BranchLine b={b} />
            {selBranch === b.name && unfold && !wide ? <BranchDetail ctx={ctx} b={b} /> : null}
          </Row>
        ))}
      </ul>
    </Panel>
  );

  const commitsPanel = (
    <Panel
      n={4}
      title={t('git.commits')}
      summary={commits[0] ? `${commits[0].short} ${commits[0].subject}` : undefined}
      focus={focus}
      wide={wide}
      setFocus={setFocus}
      grow
      extra={
        <span className="flex gap-1 text-[11px]">
          {(
            [
              [true, t('git.allBranches')],
              [false, git.branch ?? 'HEAD'],
            ] as [boolean, string][]
          ).map(([v, label]) => (
            <button key={String(v)} type="button" onClick={() => setLogAll(v)} className={`max-w-28 truncate px-1 ${logAll === v ? 'text-accent' : 'text-muted hover:text-fg'}`}>
              {label}
            </button>
          ))}
        </span>
      }
    >
      <CommitList
        ctx={ctx}
        commits={commits}
        loading={log.loading && !log.data}
        error={log.error}
        more={!!log.data?.more}
        onMore={() => setLogLimit((l) => l + 300)}
        selected={selCommit}
        active={focus === 4}
        unfold={unfold && !wide}
        onSelect={(h) => select(4, () => setSelCommit(h), selCommit === h)}
        narrow={width < 520}
      />
    </Panel>
  );

  const stashPanel = (
    <Panel n={5} title={t('git.stash')} focus={focus} wide={wide} setFocus={setFocus} count={stashRows.length} summary={stashRows[0]?.message ?? t('git.stashEmpty')}>
      {stashRows.length ? (
        <ul>
          {stashRows.map((s) => (
            <Row key={s.ref} selected={selStash === s.ref} active={focus === 5} onClick={() => select(5, () => setSelStash(s.ref), selStash === s.ref)}>
              <span className="flex min-w-0 gap-2">
                <span className="shrink-0 text-muted">{s.ref.replace('stash@', '')}</span>
                <span className="truncate">{s.message}</span>
                <span className="ml-auto shrink-0 text-muted">{s.date}</span>
              </span>
              {selStash === s.ref && unfold && !wide ? <StashDetail ctx={ctx} s={s} /> : null}
            </Row>
          ))}
        </ul>
      ) : (
        <div className="px-3 py-2 text-[13px] text-muted">{t('git.stashEmpty')}</div>
      )}
    </Panel>
  );

  const main = (() => {
    if (focus === 2 && selectedFile) return { title: selectedFile.f.path, body: <FileDetail ctx={ctx} f={selectedFile.f} staged={selectedFile.staged} /> };
    if (focus === 3 && selectedBranch) return { title: selectedBranch.name, body: <BranchDetail ctx={ctx} b={selectedBranch} /> };
    const c = commits.find((x) => x.hash === selCommit);
    if (focus === 4 && c) return { title: `${c.short} ${c.subject}`, body: <CommitDetails ctx={ctx} c={c} /> };
    if (focus === 5 && selectedStash) return { title: selectedStash.ref, body: <StashDetail ctx={ctx} s={selectedStash} /> };
    return { title: t('git.main'), body: <div className="px-3 py-4 text-[13px] text-muted">{t('git.mainHint')}</div> };
  })();

  return (
    <div ref={rootRef} tabIndex={0} onKeyDown={onKey} className="w-full px-3 py-4 font-mono text-[13px] outline-none">
      <div className={wide ? 'grid grid-cols-[minmax(0,5fr)_minmax(0,6fr)] gap-x-3' : ''}>
        <div className="space-y-4">
          <StatusPanel ctx={ctx} focus={focus} setFocus={setFocus} result={result} clearResult={() => setResult(null)} op={op} />
          {filesPanel}
          {branchesPanel}
          {commitsPanel}
          {stashPanel}
        </div>
        {wide ? (
          <div className="sticky top-4 self-start">
            <Panel n={0} title={main.title} focus={focus} wide setFocus={() => {}} main>
              <div className="max-h-[calc(100dvh-9rem)] overflow-y-auto px-3">{main.body}</div>
            </Panel>
          </div>
        ) : null}
      </div>
      <KeyHints focus={focus} />
    </div>
  );
}



