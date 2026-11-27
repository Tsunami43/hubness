import type { GitOpResult, GitStatus } from '@hubness/shared';

export type OpResult = GitOpResult & { name: string; retry: () => void };
export type Run = (op: string, label: string, body?: Record<string, unknown>) => Promise<GitOpResult>;

export interface Ctx {
  projectId: string;
  git: GitStatus;
  run: Run;
  busy: boolean;
}

