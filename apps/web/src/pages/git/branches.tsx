import type { GitBranch as Branch } from '@hubness/shared';
import { t } from '../../i18n';
import { deleteBranch, mergeBranch, rebaseBranch, switchBranch } from './actions';
import { type Ctx } from './types';
import { Act, Acts, Detail } from './ui';

// ---------- [3] branches ----------

export function BranchLine({ b }: { b: Branch }) {
  const slash = b.remote ? b.name.indexOf('/') : -1;
  return (
    <span className="flex min-w-0 gap-2">
      <span className="w-3 shrink-0 text-accent">{b.current ? '*' : ''}</span>
      <span className={`min-w-0 truncate ${b.current ? 'font-semibold text-accent' : b.remote ? '' : 'text-fg'}`}>
        {slash > 0 ? (
          <>
            <span className="text-muted">{b.name.slice(0, slash + 1)}</span>
            {b.name.slice(slash + 1)}
          </>
        ) : (
          b.name
        )}
      </span>
      <span className="ml-auto shrink-0 text-[12px] text-muted">{b.lastCommit.replace(/ ago$/, '')}</span>
    </span>
  );
}

export function BranchDetail({ ctx, b }: { ctx: Ctx; b: Branch }) {
  const current = ctx.git.branch;
  return (
    <Detail>
      {b.current ? (
        <div className="text-muted">
          {t('git.current')} · {b.upstream ? `→ ${b.upstream}` : t('git.noUpstream')}
        </div>
      ) : (
        <Acts>
          <Act k="space" disabled={ctx.busy} onClick={() => switchBranch(ctx.run, b.name)}>
            {t('git.switch')}
          </Act>
          {current ? (
            <>
              <Act k="M" disabled={ctx.busy} onClick={() => mergeBranch(ctx.run, b.name, current)}>
                {t('git.mergeInto', { branch: current })}
              </Act>
              <Act k="r" disabled={ctx.busy} onClick={() => rebaseBranch(ctx.run, current, b.name)}>
                {t('git.rebaseOnto', { branch: current })}
              </Act>
            </>
          ) : null}
          {b.remote ? null : (
            <Act k="d" danger disabled={ctx.busy} onClick={() => deleteBranch(ctx.run, b.name)}>
              {t('common.delete')}
            </Act>
          )}
        </Acts>
      )}
      <div className="text-[12px] text-muted">{b.lastCommit}</div>
    </Detail>
  );
}
