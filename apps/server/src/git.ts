import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import type { GitBranch, GitCommit, GitFile, GitLogResponse, GitOpResult, GitRepoState, GitStash, GitStatus } from '@hubness/shared';
import { childEnv } from './config.ts';
import { authProblem, gitAuthFor } from './git-auth.ts';
import { parseRemote } from './git-providers.ts';

interface ExecResult {
  code: number;
  stdout: string;
  stderr: string;
}

function git(cwd: string, args: string[], timeout = 30_000, extraEnv: Record<string, string> = {}): Promise<ExecResult> {
  return new Promise((resolve) => {
    execFile(
      'git',
      args,
      {
        cwd,
        timeout,
        maxBuffer: 32 * 1024 * 1024,
        // GIT_EDITOR=true: --continue and merges keep the prepared message instead of waiting for an editor
        env: { ...childEnv(), GIT_TERMINAL_PROMPT: '0', GIT_OPTIONAL_LOCKS: '0', GIT_EDITOR: 'true', LC_ALL: 'C', ...extraEnv },
      },
      (err, stdout, stderr) => {
        const code = err ? (typeof (err as any).code === 'number' ? (err as any).code : 1) : 0;
        resolve({ code, stdout: String(stdout), stderr: String(stderr || (err && !stderr ? err.message : '')) });
      },
    );
  });
}

const op = (r: ExecResult): GitOpResult => ({
  ok: r.code === 0,
  output: (r.stdout + (r.stderr ? (r.stdout ? '\n' : '') + r.stderr : '')).trim(),
});

/** Fetch and push URLs of all remotes. */
async function remoteUrls(cwd: string): Promise<string[]> {
  const r = await git(cwd, ['config', '--get-regexp', '^remote\\..*\\.(url|pushurl)$']);
  return r.stdout
    .split('\n')
    .map((l) => l.slice(l.indexOf(' ') + 1).trim())
    .filter(Boolean);
}

/** Fetch / pull / push: with the stored logins of the remotes' hosts; an access refusal is recognised. */
async function networkOp(cwd: string, args: string[]): Promise<GitOpResult> {
  const urls = await remoteUrls(cwd);
  const auth = gitAuthFor(urls);
  const res = op(await git(cwd, [...auth.args, ...args], 120_000, auth.env));
  if (res.ok) return res;
  const hosts = [...new Set(urls.map((u) => parseRemote(u)?.host).filter((h): h is string => Boolean(h)))];
  const problem = authProblem(res.output, hosts);
  return problem ? { ...res, auth: problem } : res;
}

export async function isRepo(cwd: string): Promise<boolean> {
  const r = await git(cwd, ['rev-parse', '--is-inside-work-tree']);
  return r.code === 0 && r.stdout.trim() === 'true';
}

export async function status(cwd: string): Promise<GitStatus> {
  const empty: GitStatus = {
    isRepo: false,
    branch: null,
    upstream: null,
    ahead: 0,
    behind: 0,
    detached: false,
    files: [],
    state: null,
    stashes: 0,
  };
  const r = await git(cwd, ['status', '--porcelain=v2', '--branch', '-z', '--untracked-files=all']);
  if (r.code !== 0) return empty;
  const [gitDir, stash] = await Promise.all([git(cwd, ['rev-parse', '--absolute-git-dir']), git(cwd, ['stash', 'list', '--format=%gd'])]);
  const st: GitStatus = { ...empty, isRepo: true, state: repoState(gitDir.stdout.trim()), stashes: stash.stdout.split('\n').filter(Boolean).length };
  const parts = r.stdout.split('\0');
  for (let i = 0; i < parts.length; i++) {
    const line = parts[i];
    if (!line) continue;
    if (line.startsWith('# branch.head ')) {
      const h = line.slice(14);
      st.detached = h === '(detached)';
      st.branch = st.detached ? null : h;
    } else if (line.startsWith('# branch.upstream ')) st.upstream = line.slice(18);
    else if (line.startsWith('# branch.ab ')) {
      const m = line.match(/\+(\d+) -(\d+)/);
      if (m) (st.ahead = Number(m[1])), (st.behind = Number(m[2]));
    } else if (line.startsWith('1 ')) {
      const f = line.split(' ');
      st.files.push(file(f[1], f.slice(8).join(' ')));
    } else if (line.startsWith('2 ')) {
      const f = line.split(' ');
      const g = file(f[1], f.slice(9).join(' '));
      g.from = parts[++i];
      st.files.push(g);
    } else if (line.startsWith('u ')) {
      const f = line.split(' ');
      st.files.push({ ...file(f[1], f.slice(10).join(' ')), conflicted: true });
    } else if (line.startsWith('? ')) {
      st.files.push({ path: line.slice(2), index: ' ', worktree: '?', untracked: true, conflicted: false });
    }
  }
  return st;
}

/** Half-done operation, from the marker files git leaves in .git. */
function repoState(gitDir: string): GitRepoState | null {
  if (!gitDir) return null;
  const has = (f: string) => existsSync(join(gitDir, f));
  if (has('rebase-merge') || has('rebase-apply')) return 'rebase';
  if (has('MERGE_HEAD')) return 'merge';
  if (has('CHERRY_PICK_HEAD')) return 'cherry-pick';
  if (has('REVERT_HEAD')) return 'revert';
  return null;
}

function file(xy: string, path: string): GitFile {
  const norm = (c: string) => (c === '.' ? ' ' : c);
  return { path, index: norm(xy[0]), worktree: norm(xy[1]), untracked: false, conflicted: false };
}

/** Unified diff of a single file or of all changes. */
export async function diff(cwd: string, opts: { path?: string; staged?: boolean; untracked?: boolean }): Promise<string> {
  if (opts.path && opts.untracked) {
    const r = await git(cwd, ['diff', '--no-index', '--no-color', '--', '/dev/null', opts.path]);
    return r.stdout; // exit code 1 means "there are differences"
  }
  const args = ['diff', '--no-color', '--no-ext-diff'];
  if (opts.staged) args.push('--cached');
  if (opts.path) args.push('--', opts.path);
  const r = await git(cwd, args);
  return r.stdout;
}

export async function branches(cwd: string): Promise<GitBranch[]> {
  const r = await git(cwd, [
    'for-each-ref',
    '--sort=-committerdate',
    '--format=%(refname)%00%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(committerdate:relative)',
    'refs/heads',
    'refs/remotes',
  ]);
  if (r.code !== 0) return [];
  return r.stdout
    .split('\n')
    .filter(Boolean)
    .map((l) => l.split('\0'))
    .filter(([ref]) => !ref.endsWith('/HEAD'))
    .map(([ref, name, head, upstream, date]) => ({
      name,
      current: head === '*',
      remote: ref.startsWith('refs/remotes/'),
      upstream: upstream || null,
      lastCommit: date,
    }));
}

export async function log(cwd: string, limit = 150, all = true): Promise<GitLogResponse> {
  const S = '\x1f';
  const r = await git(cwd, [
    'log',
    ...(all ? ['--all'] : []),
    '--topo-order',
    '--date=relative',
    `--format=%H${S}%h${S}%P${S}%s${S}%an${S}%ad${S}%D`,
    '-n',
    String(limit + 1),
  ]);
  if (r.code !== 0) return { commits: [], more: false };
  const commits = r.stdout
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const [hash = '', short = '', parents = '', subject = '', author = '', date = '', refs = ''] = line.split(S);
      return { hash, short, parents: parents.split(' ').filter(Boolean), subject, author, date, refs: refs ? refs.split(', ') : [] };
    });
  return { commits: commits.slice(0, limit), more: commits.length > limit };
}

/** Patch of one commit (first parent for merges) or one stash entry. */
export async function show(cwd: string, rev: string): Promise<string> {
  if (!validRev(rev)) return '';
  const r = rev.startsWith('stash@')
    ? await git(cwd, ['stash', 'show', '-p', '--include-untracked', '--no-color', rev])
    : await git(cwd, ['show', '--format=', '--no-color', '--diff-merges=first-parent', rev]);
  return r.stdout;
}

export async function stashes(cwd: string): Promise<GitStash[]> {
  const S = '\x1f';
  const r = await git(cwd, ['stash', 'list', `--format=%gd${S}%gs${S}%cr`]);
  if (r.code !== 0) return [];
  return r.stdout
    .split('\n')
    .filter(Boolean)
    .map((l) => {
      const [ref = '', message = '', date = ''] = l.split(S);
      return { ref, message: message.replace(/^On [^:]+: /, ''), date };
    });
}

/** Branch / commit / stash names from the UI: no options, no shell-ish characters. */
export const validRev = (rev: string) => /^[\w./@{}^~-]{1,200}$/.test(rev) && !rev.startsWith('-') && !rev.includes('..');
const bad = (what: string): GitOpResult => ({ ok: false, output: `Invalid ${what}` });

const CONTINUE: Record<GitRepoState, string> = { merge: 'merge', rebase: 'rebase', 'cherry-pick': 'cherry-pick', revert: 'revert' };

export const ops = {
  fetch: (cwd: string) => networkOp(cwd, ['fetch', '--all', '--prune']),
  pull: (cwd: string) => networkOp(cwd, ['pull', '--ff-only']),
  async push(cwd: string) {
    const st = await status(cwd);
    if (!st.branch) return { ok: false, output: 'Detached HEAD — nothing to push' };
    if (st.upstream) return networkOp(cwd, ['push']);
    return networkOp(cwd, ['push', '-u', 'origin', st.branch]);
  },
  async commit(cwd: string, message: string, all: boolean) {
    if (!message.trim()) return { ok: false, output: 'Empty commit message' };
    if (all) {
      const a = await git(cwd, ['add', '-A']);
      if (a.code !== 0) return op(a);
    }
    return op(await git(cwd, ['commit', '-m', message]));
  },
  stage: async (cwd: string, paths: string[]) => op(await git(cwd, ['add', '--', ...paths])),
  async unstage(cwd: string, paths: string[]) {
    const r = await git(cwd, ['restore', '--staged', '--', ...paths]);
    if (r.code === 0) return op(r);
    // repo without commits — no HEAD yet
    return op(await git(cwd, ['rm', '--cached', '-r', '--quiet', '--', ...paths]));
  },
  async checkout(cwd: string, branch: string, create: boolean) {
    if (create) return op(await git(cwd, ['switch', '-c', branch]));
    const remote = branch.match(/^([^/]+)\/(.+)$/);
    if (remote) {
      const local = await git(cwd, ['rev-parse', '--verify', '--quiet', `refs/heads/${remote[2]}`]);
      const isRemote = await git(cwd, ['rev-parse', '--verify', '--quiet', `refs/remotes/${branch}`]);
      if (local.code !== 0 && isRemote.code === 0) return op(await git(cwd, ['switch', '--track', branch]));
    }
    return op(await git(cwd, ['switch', branch]));
  },
  async branchAt(cwd: string, name: string, rev: string) {
    if (!validRev(name) || !validRev(rev)) return bad('branch or commit');
    return op(await git(cwd, ['switch', '-c', name, rev]));
  },
  async deleteBranch(cwd: string, branch: string, force: boolean) {
    if (!validRev(branch)) return bad('branch');
    return op(await git(cwd, ['branch', force ? '-D' : '-d', branch]));
  },
  async merge(cwd: string, branch: string, noFf: boolean) {
    if (!validRev(branch)) return bad('branch');
    return op(await git(cwd, ['merge', '--no-edit', ...(noFf ? ['--no-ff'] : []), branch], 120_000));
  },
  async rebase(cwd: string, onto: string) {
    if (!validRev(onto)) return bad('branch');
    return op(await git(cwd, ['rebase', onto], 120_000));
  },
  async reset(cwd: string, rev: string, mode: 'soft' | 'mixed' | 'hard') {
    if (!validRev(rev)) return bad('commit');
    if (!['soft', 'mixed', 'hard'].includes(mode)) return bad('mode');
    return op(await git(cwd, ['reset', `--${mode}`, rev]));
  },
  async cherryPick(cwd: string, rev: string) {
    if (!validRev(rev)) return bad('commit');
    return op(await git(cwd, ['cherry-pick', rev]));
  },
  async revert(cwd: string, rev: string) {
    if (!validRev(rev)) return bad('commit');
    return op(await git(cwd, ['revert', '--no-edit', rev]));
  },
  /** Continue or abort whatever operation is half-done. */
  async proceed(cwd: string, action: 'continue' | 'abort') {
    const st = await status(cwd);
    if (!st.state) return { ok: false, output: 'Nothing in progress' };
    if (action === 'continue' && st.files.some((f) => f.conflicted)) return { ok: false, output: 'Resolve and stage all conflicted files first' };
    return op(await git(cwd, [CONTINUE[st.state], `--${action}`], 120_000));
  },
  /** Throw away local changes of files: tracked ones back to HEAD, untracked ones deleted. */
  async discard(cwd: string, paths: string[]) {
    const st = await status(cwd);
    const set = new Set(paths);
    const untracked = st.files.filter((f) => set.has(f.path) && f.untracked).map((f) => f.path);
    const tracked = paths.filter((p) => !untracked.includes(p));
    if (tracked.length) {
      const r = await git(cwd, ['restore', '--source=HEAD', '--staged', '--worktree', '--', ...tracked]);
      if (r.code !== 0) return op(r);
    }
    if (untracked.length) return op(await git(cwd, ['clean', '-f', '-q', '--', ...untracked]));
    return { ok: true, output: '' };
  },
  async stashPush(cwd: string, message: string, untracked: boolean) {
    return op(await git(cwd, ['stash', 'push', ...(untracked ? ['--include-untracked'] : []), ...(message.trim() ? ['-m', message.trim()] : [])]));
  },
  async stash(cwd: string, action: 'apply' | 'pop' | 'drop', ref: string) {
    if (!/^stash@\{\d+\}$/.test(ref)) return bad('stash');
    return op(await git(cwd, ['stash', action, ref]));
  },
};

/** Repository info: remote origin, current branch, default branch (origin/HEAD, else main/master). */
export async function repoBasics(cwd: string): Promise<{ isRepo: boolean; remoteUrl: string | null; branch: string | null; defaultBranch: string | null }> {
  if (!(await isRepo(cwd))) return { isRepo: false, remoteUrl: null, branch: null, defaultBranch: null };
  const [remote, branch, head] = await Promise.all([
    git(cwd, ['remote', 'get-url', 'origin']),
    git(cwd, ['symbolic-ref', '--quiet', '--short', 'HEAD']),
    git(cwd, ['symbolic-ref', '--quiet', '--short', 'refs/remotes/origin/HEAD']),
  ]);
  let defaultBranch = head.code === 0 ? head.stdout.trim().replace(/^origin\//, '') : null;
  if (!defaultBranch) {
    for (const b of ['main', 'master']) {
      const r = await git(cwd, ['rev-parse', '--verify', '--quiet', `refs/heads/${b}`]);
      if (r.code === 0) {
        defaultBranch = b;
        break;
      }
    }
  }
  return {
    isRepo: true,
    remoteUrl: remote.code === 0 ? remote.stdout.trim() || null : null,
    branch: branch.code === 0 ? branch.stdout.trim() || null : null,
    defaultBranch,
  };
}

