// Git panel: status, branches, history, stashes, operations, logins per host.

// ---- Git ----

export interface GitFile {
  path: string;
  /** For renames — the old path. */
  from?: string;
  /** Index status letter (X from porcelain) or ' '. */
  index: string;
  /** Working tree status letter (Y) or ' '. */
  worktree: string;
  untracked: boolean;
  conflicted: boolean;
}

export interface GitStatus {
  isRepo: boolean;
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  detached: boolean;
  files: GitFile[];
  /** An operation stopped half-way (conflicts or a paused rebase). */
  state: GitRepoState | null;
  /** Number of stash entries. */
  stashes: number;
}

export type GitRepoState = 'merge' | 'rebase' | 'cherry-pick' | 'revert';

export interface GitStash {
  /** stash@{0} */
  ref: string;
  message: string;
  date: string;
}

export interface GitBranch {
  name: string;
  current: boolean;
  remote: boolean;
  upstream: string | null;
  lastCommit: string;
}

export interface GitCommit {
  hash: string;
  short: string;
  /** Parent hashes (two or more — a merge); the client draws the graph from them. */
  parents: string[];
  subject: string;
  author: string;
  date: string;
  /** Decorations from %D: `HEAD -> main`, `origin/main`, `tag: v1`. */
  refs: string[];
}

export interface GitLogResponse {
  commits: GitCommit[];
  /** More commits exist beyond the limit. */
  more: boolean;
}

export interface GitOpResult {
  ok: boolean;
  output: string;
  /** Set when a network operation failed because the remote refused access. */
  auth?: GitAuthProblem;
}

/**
 * Why the remote refused: missing — no login stored for the host (https) or no key
 * accepted (ssh); rejected — the stored login was refused; hostkey — the ssh host is
 * not in known_hosts.
 */
export interface GitAuthProblem {
  host: string;
  protocol: 'https' | 'ssh';
  reason: 'missing' | 'rejected' | 'hostkey';
}

export type GitCredentialKind = 'token' | 'password';

/** Stored git login for one host (the secret itself never leaves the server). */
export interface GitCredentialInfo {
  host: string;
  username: string;
  kind: GitCredentialKind;
  updatedAt: string;
}
