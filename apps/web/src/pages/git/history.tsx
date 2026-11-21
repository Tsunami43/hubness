import { useMemo, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import type { GitCommit, GitStash } from '@hubness/shared';
import { api } from '../../lib/api';
import { confirmDialog } from '../../components/Confirm';
import { useApi } from '../../lib/query';
import { layoutGraph, laneColor, type GraphRow } from '../../lib/gitGraph';
import { DiffView } from '../../components/DiffView';
import { Button, ErrorLine, inputCls } from '../../components/ui';
import { t } from '../../i18n';
import { dropStash } from './actions';
import { type Ctx } from './types';
import { Act, Acts, Busy, Detail, Row } from './ui';

// ---------- [4] commits ----------

export const ROW_H = 22;
export const LANE_W = 11;
export const PAD = 7;
export const R = 5;
export const laneX = (i: number) => PAD + i * LANE_W;

/** Box-drawing style graph cell (│ ╮ ╯ ├ ◯), drawn as SVG so lines never break between rows. */
export function GraphCell({ row, width, head, max }: { row: GraphRow; width: number; head: boolean; max: number }) {
  const mid = ROW_H / 2;
  const x = laneX(row.col);
  const node = row.merge ? 4 : 3.5;
  const paths: { d: string; color: string }[] = [];
  const visible = (i: number) => i < max;
  row.above.forEach((h, i) => {
    if (visible(i) && h && i !== row.col && !row.childCols.includes(i) && row.below[i] === h) paths.push({ d: `M${laneX(i)} 0V${ROW_H}`, color: laneColor(i) });
  });
  if (!row.tip && row.childCols.includes(row.col)) paths.push({ d: `M${x} 0V${mid - node}`, color: laneColor(row.col) });
  if (row.parentCols.includes(row.col)) paths.push({ d: `M${x} ${mid + node}V${ROW_H}`, color: laneColor(row.col) });
  for (const i of row.childCols) {
    if (i === row.col || !visible(i)) continue;
    const xi = laneX(i);
    const dir = i > row.col ? -1 : 1;
    // ╯ / ╰ : down from the top, turn towards the node
    paths.push({ d: `M${xi} 0V${mid - R}Q${xi} ${mid} ${xi + dir * R} ${mid}H${x - dir * node}`, color: laneColor(i) });
  }
  for (const k of row.parentCols) {
    if (k === row.col || !visible(k)) continue;
    const xk = laneX(k);
    const dir = k > row.col ? 1 : -1;
    const through = row.above[k] && row.above[k] === row.below[k];
    // ├ / ┤ into an existing lane, ╮ / ╭ opening a new one
    paths.push({
      d: through ? `M${x + dir * node} ${mid}H${xk}` : `M${x + dir * node} ${mid}H${xk - dir * R}Q${xk} ${mid} ${xk} ${mid + R}V${ROW_H}`,
      color: laneColor(k),
    });
  }
  const color = laneColor(row.col);
  return (
    <svg width={width} height={ROW_H} className="block shrink-0" aria-hidden>
      {paths.map((p, i) => (
        <path key={i} d={p.d} stroke={p.color} strokeWidth={1.6} fill="none" />
      ))}
      {visible(row.col) ? (
        row.merge ? (
          <rect x={x - node} y={mid - node} width={node * 2} height={node * 2} fill="none" stroke={color} strokeWidth={1.6} transform={`rotate(45 ${x} ${mid})`} />
        ) : (
          <circle cx={x} cy={mid} r={node} fill={head ? color : 'none'} stroke={color} strokeWidth={1.6} />
        )
      ) : null}
    </svg>
  );
}

/** Lanes continuing past an unfolded row. */
export function GraphTail({ row, width, max }: { row: GraphRow; width: number; max: number }) {
  return (
    <svg width={width} className="block min-h-0 flex-1" preserveAspectRatio="none" viewBox={`0 0 ${width} 10`} aria-hidden>
      {row.below.map((h, i) =>
        h && i < max ? <path key={i} d={`M${laneX(i)} 0V10`} stroke={laneColor(i)} strokeWidth={1.6} vectorEffect="non-scaling-stroke" /> : null,
      )}
    </svg>
  );
}

export const initials = (name: string) =>
  name
    .split(/[\s._-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]!.toUpperCase())
    .join('')
    .padEnd(2, ' ');

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

export function CommitList({
  ctx,
  commits,
  loading,
  error,
  more,
  onMore,
  selected,
  active,
  unfold,
  onSelect,
  narrow,
}: {
  narrow: boolean;
  ctx: Ctx;
  commits: GitCommit[];
  loading: boolean;
  error: string | null;
  more: boolean;
  onMore: () => void;
  selected: string | null;
  active: boolean;
  unfold: boolean;
  onSelect: (hash: string) => void;
}) {
  const layout = useMemo(() => layoutGraph(commits), [commits]);
  // phones get fewer lanes so subjects stay readable (lazygit clips wide graphs too)
  const max = narrow ? 4 : 10;
  const width = PAD * 2 + (Math.min(layout.width, max) - 1) * LANE_W;
  if (error) return <ErrorLine error={error} />;
  if (loading) return <Busy />;
  if (!commits.length) return <div className="px-3 py-2 text-muted">{t('git.noCommits')}</div>;
  return (
    <>
      <ul>
        {commits.map((c, i) => {
          const row = layout.rows[i];
          const refs = c.refs.filter((r) => r !== 'origin/HEAD');
          const head = c.refs.some((r) => r === 'HEAD' || r.startsWith('HEAD -> '));
          const open = unfold && selected === c.hash;
          return (
            <Row key={c.hash} selected={selected === c.hash} active={active} onClick={() => onSelect(c.hash)}>
              <div className="-mx-3 flex">
                <span className="shrink-0 pr-2 pl-3 text-warn">{c.short}</span>
                {narrow ? null : (
                  <span className="w-7 shrink-0" style={{ color: authorColor(c.author) }} title={c.author}>
                    {initials(c.author)}
                  </span>
                )}
                <div className="flex shrink-0 flex-col" style={{ width }}>
                  <GraphCell row={row} width={width} head={head} max={max} />
                  {open && !narrow ? <GraphTail row={row} width={width} max={max} /> : null}
                </div>
                <div className="min-w-0 flex-1 pr-3">
                  <div className="flex min-w-0 gap-1.5" style={{ height: ROW_H }}>
                    {refs.slice(0, 2).map((r) => (
                      <RefTag key={r} name={r} />
                    ))}
                    {refs.length > 2 ? <span className="shrink-0 text-muted">+{refs.length - 2}</span> : null}
                    <span className={`truncate ${head ? 'text-fg' : 'text-fg/90'}`}>{c.subject}</span>
                  </div>
                  {open && !narrow ? <CommitDetails ctx={ctx} c={c} /> : null}
                </div>
              </div>
              {/* narrow (phone, sidebar): details take the full row width */}
              {open && narrow ? <CommitDetails ctx={ctx} c={c} /> : null}
            </Row>
          );
        })}
      </ul>
      {more ? (
        <button type="button" onClick={onMore} className="w-full py-1 text-accent hover:bg-sunken">
          {t('git.loadMore')}
        </button>
      ) : null}
    </>
  );
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
