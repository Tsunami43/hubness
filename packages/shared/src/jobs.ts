// Terminal jobs: install / login / update commands, git clone, project shells.

/** Terminal task (install, login, update). */
export interface JobInfo {
  id: string;
  title: string;
  command: string;
  status: 'running' | 'exited';
  exitCode: number | null;
  startedAt: string;
}

export type JobStreamMsg =
  | { type: 'snapshot'; job: JobInfo; output: string }
  | { type: 'output'; data: string }
  | { type: 'exit'; code: number | null };
