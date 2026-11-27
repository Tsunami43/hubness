import type { GitFile } from '@hubness/shared';

/**
 * Letter and colour of a file's change. `side`: the staged half (index), the unstaged half
 * (worktree), or `any` — one row per file, the worktree change first (the phone's list).
 */
export function gitMark(f: Pick<GitFile, 'index' | 'worktree' | 'untracked' | 'conflicted'>, side: 'staged' | 'worktree' | 'any'): { letter: string; cls: string } {
  if (f.conflicted) return { letter: '!', cls: 'text-danger' };
  if (f.untracked) return { letter: 'U', cls: 'text-ok' };
  const x = side === 'staged' ? f.index : side === 'worktree' ? f.worktree : f.worktree !== ' ' ? f.worktree : f.index;
  if (x === 'A') return { letter: 'A', cls: 'text-ok' };
  if (x === 'D') return { letter: 'D', cls: 'text-danger' };
  if (x === 'R') return { letter: 'R', cls: 'text-accent' };
  return { letter: 'M', cls: 'text-warn' };
}
