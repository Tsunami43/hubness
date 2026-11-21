import type { GitFile, GitOpResult, GitStatus } from '@hubness/shared';

export type PanelId = 1 | 2 | 3 | 4 | 5;
export type OpResult = GitOpResult & { name: string; retry: () => void };
export type Run = (op: string, label: string, body?: Record<string, unknown>) => Promise<GitOpResult>;

export interface Ctx {
  projectId: string;
  git: GitStatus;
  run: Run;
  busy: boolean;
}

export const fileKey = (f: GitFile, staged: boolean) => `${staged ? 's' : 'w'}:${f.path}`;
