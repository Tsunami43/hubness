import { createContext, useContext } from 'react';
import type { GitStatus } from '@hubness/shared';

/** Repository = project: project id, name, path. */
export interface Repo {
  id: string;
  name: string;
  path: string;
}

/** Current repository for the Git panel (in a group — the selected member). */
export interface RepoCtx {
  repo: Repo;
  git: GitStatus | null;
  refreshGit: () => void;
}

export const RepoContext = createContext<RepoCtx | null>(null);

export function useRepo(): RepoCtx {
  const c = useContext(RepoContext);
  if (!c) throw new Error('useRepo outside RepoContext');
  return c;
}
