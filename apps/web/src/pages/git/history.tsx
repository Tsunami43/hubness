import { useState } from 'react';
import { Check, Copy } from 'lucide-react';
import type { GitCommit, GitStash } from '@hubness/shared';
import { api } from '../../lib/api';
import { confirmDialog } from '../../components/Confirm';
import { useApi } from '../../lib/query';
import { laneColor } from '../../lib/gitGraph';
import { DiffView } from '../../components/DiffView';
import { Button, ErrorLine, inputCls } from '../../components/ui';
import { t } from '../../i18n';
import { dropStash } from './actions';
import { type Ctx } from './types';
import { Act, Acts, Busy, Detail } from './ui';

export function authorColor(name: string) {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) | 0;
  return laneColor(Math.abs(h) + 1);
}

export function RefTag({ name }: { name: string }) {
  const head = name.startsWith('HEAD -> ') || name === 'HEAD';
  const label = name.replace('HEAD -> ', '').replace(/^tag: /, '');
  const cls = name.startsWith('tag: ') ? 'text-warn' : head ? 'font-semibold text-accent' : name.includes('/') ? 'text-muted' : 'text-ok';
  return <span className={`max-w-44 shrink-0 truncate ${cls}`}>{head && name !== 'HEAD' ? `*${label}` : label}</span>;
}

export function CommitDetails({ ctx, c }: { ctx: Ctx; c: GitCommit }) {
  const { run, busy, git } = ctx;
  const [panel, setPanel] = useState<'reset' | 'branch' | null>(null);
  const [name, setName] = useState('');
  const [copied, setCopied] = useState(false);
  const target = git.branch ?? 'HEAD';
  const toggle = (p: typeof panel) => setPanel(panel === p ? null : p);
  const patch = useApi(['git', ctx.projectId, 'show', c.hash], () => api.gitShow(ctx.projectId, c.hash));

  const reset = async (mode: 'soft' | 'mixed' | 'hard') => {
    const message = t('git.resetConfirm', { branch: target, commit: c.short }) + (mode === 'hard' ? `\n${t('git.resetHardWarn')}` : '');
    if (await confirmDialog({ message, confirmLabel: `reset --${mode}`, danger: mode === 'hard' })) {
      if ((await run('reset', `Reset --${mode} ${c.short}`, { rev: c.hash, mode })).ok) setPanel(null);
    }
  };

  return (
    <Detail>
      <div className="space-y-0.5 text-[12px] leading-5 text-muted">
        <div className="flex items-center gap-2">
          <span className="truncate text-warn">{c.hash}</span>
          <button
            type="button"
            className="shrink-0 hover:text-fg"
            title={t('git.copyHash')}
            onClick={() => {
              void navigator.clipboard?.writeText(c.hash);
              setCopied(true);
              setTimeout(() => setCopied(false), 1200);
            }}
          >
            {copied ? <Check className="size-3.5 text-ok" /> : <Copy className="size-3.5" />}
          </button>
        </div>
        <div className="text-fg">{c.subject}</div>
        <div>
          <span style={{ color: authorColor(c.author) }}>{c.author}</span> · {c.date}
          {c.parents.length > 1 ? ` · ${t('git.mergeCommit')}` : ''}
        </div>
        {c.refs.length ? (
          <div className="flex flex-wrap gap-x-2">
            {c.refs.map((r) => (
              <RefTag key={r} name={r} />
            ))}
          </div>
        ) : null}
      </div>
      <Acts>
        <Act onClick={() => toggle('branch')}>{t('git.branchHere')}</Act>
        <Act disabled={busy} onClick={() => run('cherry-pick', `Cherry-pick ${c.short}`, { rev: c.hash })}>
          cherry-pick
        </Act>
        <Act
          disabled={busy}
          onClick={async () => {
            if (await confirmDialog({ message: t('git.revertConfirm', { commit: c.short }), confirmLabel: 'revert' })) run('revert', `Revert ${c.short}`, { rev: c.hash });
          }}
        >
          revert
        </Act>
        <Act onClick={() => toggle('reset')}>reset…</Act>
      </Acts>
      {panel === 'branch' ? (
        <form
          className="flex gap-1.5 py-1"
          onSubmit={async (e) => {
            e.preventDefault();
            if (name.trim() && (await run('branch-at', t('git.switching', { branch: name.trim() }), { branch: name.trim(), rev: c.hash })).ok) setPanel(null);
          }}
        >
          <input className={`${inputCls} h-7 py-0 font-mono text-[13px]`} placeholder={t('git.newBranch')} value={name} onChange={(e) => setName(e.target.value)} autoCapitalize="off" autoFocus />
          <Button type="submit" size="sm" variant="primary" disabled={!name.trim() || busy}>
            {t('git.create')}
          </Button>
        </form>
      ) : null}
      {panel === 'reset' ? (
        <div className="my-1 border border-line">
          <div className="px-2 py-1 text-[12px] text-muted">{t('git.resetTitle', { branch: target, commit: c.short })}</div>
          {(
            [
              ['soft', t('git.resetSoft')],
              ['mixed', t('git.resetMixed')],
              ['hard', t('git.resetHard')],
            ] as const
          ).map(([mode, hint]) => (
            <button key={mode} type="button" disabled={busy} onClick={() => reset(mode)} className="flex w-full items-baseline gap-3 px-2 leading-6 hover:bg-sunken">
              <span className={`w-10 shrink-0 ${mode === 'hard' ? 'text-danger' : 'text-accent'}`}>{mode}</span>
              <span className="text-[12px] text-muted">{hint}</span>
            </button>
          ))}
        </div>
      ) : null}
      <div className="mt-1">{patch.error ? <ErrorLine error={patch.error} /> : !patch.data ? <Busy /> : <DiffView diff={patch.data.diff} maxHeight="55dvh" />}</div>
    </Detail>
  );
}

// ---------- [5] stash ----------

export function StashDetail({ ctx, s }: { ctx: Ctx; s: GitStash }) {
  const patch = useApi(['git', ctx.projectId, 'show', s.ref], () => api.gitShow(ctx.projectId, s.ref));
  return (
    <Detail>
      <div className="text-[12px] text-muted">
        {s.message} · {s.date}
      </div>
      <Acts>
        <Act k="space" disabled={ctx.busy} onClick={() => ctx.run('stash', `Stash apply ${s.ref}`, { action: 'apply', rev: s.ref })}>
          {t('git.stashApply')}
        </Act>
        <Act k="g" disabled={ctx.busy} onClick={() => ctx.run('stash', `Stash pop ${s.ref}`, { action: 'pop', rev: s.ref })}>
          {t('git.stashPop')}
        </Act>
        <Act k="d" danger disabled={ctx.busy} onClick={() => dropStash(ctx.run, s.ref)}>
          {t('common.delete')}
        </Act>
      </Acts>
      {patch.error ? <ErrorLine error={patch.error} /> : !patch.data ? <Busy /> : <DiffView diff={patch.data.diff} maxHeight="55dvh" />}
    </Detail>
  );
}
