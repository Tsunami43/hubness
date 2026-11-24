import { useEffect, useRef, useState, type RefObject } from 'react';
import { Check } from 'lucide-react';
import type { GitFile } from '@hubness/shared';
import { api } from '../../lib/api';
import { confirmDialog } from '../../components/Confirm';
import { splitPath } from '../../lib/format';
import { useApi } from '../../lib/query';
import { DiffView } from '../../components/DiffView';
import { Button, ErrorLine, inputCls } from '../../components/ui';
import { useWorkbench } from '../../lib/workbench';
import { GitAuthNotice, GitLoginSheet } from '../../components/GitLogin';
import { t } from '../../i18n';
import { discard } from './actions';
import { type Ctx, type OpResult, type PanelId } from './types';
import { Act, Acts, Busy, Detail, Panel } from './ui';

// ---------- [1] status ----------

export function StatusPanel({
  ctx,
  focus,
  setFocus,
  result,
  clearResult,
  op,
}: {
  ctx: Ctx;
  focus: PanelId;
  setFocus: (p: PanelId) => void;
  result: OpResult | null;
  clearResult: () => void;
  op: string | null;
}) {
  const { git, run } = ctx;
  const [login, setLogin] = useState<string | null>(null);
  const conflicts = git.files.filter((f) => f.conflicted).length;
  const stateName = git.state ? t(`git.state.${git.state}` as 'git.state.merge') : null;
  return (
    <Panel n={1} title={t('git.status')} focus={focus} wide setFocus={setFocus}>
      <div className="space-y-1.5 px-3 pb-1.5">
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
          <span className="font-semibold text-accent">{git.branch ?? 'HEAD (detached)'}</span>
          {git.upstream ? <span className="text-muted">→ {git.upstream}</span> : <span className="text-muted">{t('git.noUpstream')}</span>}
          {git.ahead ? <span className="text-ok">↑{git.ahead}</span> : null}
          {git.behind ? <span className="text-warn">↓{git.behind}</span> : null}
        </div>
        <Acts>
          <Act k="f" disabled={!!op} onClick={() => run('fetch', 'Fetch')}>
            {op === 'fetch' ? '…' : 'fetch'}
          </Act>
          <Act k="p" disabled={!!op} onClick={() => run('pull', 'Pull')}>
            {op === 'pull' ? '…' : `pull${git.behind ? ` ${git.behind}` : ''}`}
          </Act>
          <Act k="P" disabled={!!op} onClick={() => run('push', 'Push')}>
            {op === 'push' ? '…' : `push${git.ahead ? ` ${git.ahead}` : ''}`}
          </Act>
        </Acts>
        {stateName ? (
          <div className="border border-warn/50 px-2 py-1.5">
            <div className="text-warn">{stateName}</div>
            <div className="text-[12px] text-muted">{conflicts ? t('git.conflicts', { n: conflicts }) : t('git.readyToContinue')}</div>
            <Acts>
              <Act disabled={ctx.busy || conflicts > 0} onClick={() => run('continue', t('git.continue'))}>
                {t('git.continue')}
              </Act>
              <Act
                danger
                disabled={ctx.busy}
                onClick={async () => {
                  if (await confirmDialog({ message: t('git.abortConfirm', { op: stateName }), confirmLabel: t('git.abort'), danger: true })) run('abort', t('git.abort'));
                }}
              >
                {t('git.abort')}
              </Act>
            </Acts>
          </div>
        ) : null}
        {result ? (
          <div className={`px-2 py-1 text-[12px] ${result.ok ? 'bg-sunken' : 'border border-danger/40 text-danger'}`}>
            <div className="flex gap-2">
              <span className="flex-1">
                {result.ok ? '✓' : '✗'} {result.name}
              </span>
              <button type="button" onClick={clearResult} className="text-muted hover:text-fg">
                ×
              </button>
            </div>
            {result.auth ? <GitAuthNotice problem={result.auth} onSignIn={() => setLogin(result.auth!.host)} /> : null}
            {result.output ? <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap">{result.output}</pre> : null}
          </div>
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
    </Panel>
  );
}

// ---------- [2] files ----------

export function FileLine({ f, staged }: { f: GitFile; staged: boolean }) {
  const { dir, name } = splitPath(f.path);
  // lazygit: X (index) green, Y (worktree) red; this row shows one side of the file
  const x = f.conflicted ? 'U' : f.untracked ? '?' : staged ? f.index : ' ';
  const y = f.conflicted ? 'U' : f.untracked ? '?' : staged ? ' ' : f.worktree;
  return (
    <span className="flex min-w-0 gap-2">
      <span className="shrink-0 whitespace-pre">
        <span className={f.conflicted ? 'text-danger' : 'text-ok'}>{x}</span>
        <span className="text-danger">{y}</span>
      </span>
      <span className="min-w-0 truncate">
        <span className="text-muted">{dir}</span>
        <span className={staged ? 'text-ok' : 'text-fg'}>{name}</span>
      </span>
    </span>
  );
}

export function FileDetail({ ctx, f, staged }: { ctx: Ctx; f: GitFile; staged: boolean }) {
  const wb = useWorkbench();
  const { data, error, reload } = useApi(['git', ctx.projectId, 'diff', f.path, staged, f.untracked], () =>
    api.gitDiff(ctx.projectId, { path: f.path, staged, untracked: f.untracked }),
  );
  // a fresh status (file edited again) means a fresh diff
  const firstStatus = useRef(ctx.git);
  useEffect(() => {
    if (ctx.git !== firstStatus.current) void reload();
  }, [ctx.git, reload]);
  return (
    <Detail>
      <Acts>
        <Act k="space" disabled={ctx.busy} onClick={() => ctx.run(staged ? 'unstage' : 'stage', staged ? t('git.unstage') : t('git.stage'), { paths: [f.path] })}>
          {staged ? t('git.unstage') : t('git.stage')}
        </Act>
        {f.conflicted ? null : (
          <Act k="d" danger disabled={ctx.busy} onClick={() => discard(ctx.run, [f.path], f.path)}>
            {t('git.discard')}
          </Act>
        )}
        {wb ? <Act onClick={() => wb.openFile(ctx.projectId, f.path, { mode: 'diff' })}>{t('git.openDiff')}</Act> : null}
      </Acts>
      {error ? <ErrorLine error={error} /> : !data ? <Busy /> : <DiffView diff={data.diff} maxHeight="55dvh" path={f.path} />}
    </Detail>
  );
}

export function CommitBox({ ctx, inputRef, stagedCount, unstagedCount }: { ctx: Ctx; inputRef: RefObject<HTMLTextAreaElement | null>; stagedCount: number; unstagedCount: number }) {
  const [message, setMessage] = useState('');
  const all = stagedCount === 0;
  if (!stagedCount && !unstagedCount) return null;
  const submit = async () => {
    if (message.trim() && (await ctx.run('commit', t('git.commit'), { message, all })).ok) setMessage('');
  };
  return (
    <div className="mt-1 border-t border-line/60 px-3 pt-2 pb-1.5">
      <textarea
        ref={inputRef}
        rows={2}
        className={`${inputCls} font-mono text-[13px]`}
        placeholder={t('git.commitMessage')}
        value={message}
        onChange={(e) => setMessage(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) void submit();
          if (e.key === 'Escape') (e.target as HTMLTextAreaElement).blur();
        }}
      />
      <div className="mt-1.5 flex gap-1.5">
        <Button className="min-w-0 flex-1" size="sm" variant="primary" disabled={!message.trim() || ctx.busy} onClick={submit}>
          <Check className="size-3.5 shrink-0" />
          <span className="truncate">{all ? t('git.commitAll', { n: unstagedCount }) : t('git.commitStaged', { n: stagedCount })}</span>
        </Button>
        <Act k="s" disabled={ctx.busy} onClick={() => ctx.run('stash-push', t('git.stashPush'), { message: message.trim(), untracked: true })}>
          stash
        </Act>
      </div>
    </div>
  );
}
